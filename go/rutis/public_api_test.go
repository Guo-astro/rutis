package rutis_test

// The public API of the Go SDK, locked by public-api.txt (#692, SDK entry
// design §6): every exported declaration of rutis, rutis/advanced and
// rutis/rutistest with its signature, methods and exported fields. A type
// that is an alias of one in internal/runtime is listed as that type, with
// its fields and methods. Adding or changing a name changes the text, and
// review sees it.
//
// To accept an intended change:
//
//	go test -run TestPublicAPI -update

import (
	"bytes"
	"flag"
	"go/ast"
	"go/doc"
	"go/parser"
	"go/printer"
	"go/token"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

var update = flag.Bool("update", false, "rewrite public-api.txt")

const module = "github.com/arcships/rutis/go/rutis"

func TestPublicAPI(t *testing.T) {
	fset := token.NewFileSet()
	internal := map[string]*doc.Type{}
	for _, typ := range parsePackage(t, fset, "internal/runtime").Types {
		internal[typ.Name] = typ
	}
	var out bytes.Buffer
	for _, dir := range []string{".", "advanced", "rutistest"} {
		path := module
		if dir != "." {
			path += "/" + dir
		}
		out.WriteString("## " + path + "\n" + strings.Join(publicAPI(fset, parsePackage(t, fset, dir), internal), "\n") + "\n\n")
	}
	if *update {
		if err := os.WriteFile("public-api.txt", out.Bytes(), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	// A Windows checkout may turn the snapshot's (and the sources') line
	// ends into CRLF: compare the lines, not the bytes.
	want, _ := os.ReadFile("public-api.txt")
	wantText := strings.ReplaceAll(string(want), "\r\n", "\n")
	got := strings.ReplaceAll(out.String(), "\r\n", "\n")
	if wantText != got {
		t.Fatalf("\n%sthe public API of rutis changed: review the diff, then run go test -run TestPublicAPI -update", diff(wantText, got))
	}
}

func parsePackage(t *testing.T, fset *token.FileSet, dir string) *doc.Package {
	t.Helper()
	names, err := filepath.Glob(filepath.Join(dir, "*.go"))
	if err != nil {
		t.Fatal(err)
	}
	var files []*ast.File
	for _, name := range names {
		if strings.HasSuffix(name, "_test.go") {
			continue
		}
		file, err := parser.ParseFile(fset, name, nil, parser.ParseComments)
		if err != nil {
			t.Fatal(err)
		}
		files = append(files, file)
	}
	pkg, err := doc.NewFromFiles(fset, files, module+"/"+dir)
	if err != nil {
		t.Fatal(err)
	}
	return pkg
}

// publicAPI lists pkg's exported declarations, one per entry; `internal`
// has the types of internal/runtime its aliases name.
func publicAPI(fset *token.FileSet, pkg *doc.Package, internal map[string]*doc.Type) []string {
	var lines []string
	values := func(list []*doc.Value) {
		for _, value := range list {
			for _, spec := range value.Decl.Specs {
				spec := spec.(*ast.ValueSpec)
				if !spec.Names[0].IsExported() {
					continue
				}
				lines = append(lines, value.Decl.Tok.String()+" "+source(fset, stripValue(spec)))
			}
		}
	}
	funcs := func(list []*doc.Func) {
		for _, fn := range list {
			lines = append(lines, source(fset, &ast.FuncDecl{Recv: fn.Decl.Recv, Name: fn.Decl.Name, Type: fn.Decl.Type}))
		}
	}
	values(pkg.Consts)
	values(pkg.Vars)
	funcs(pkg.Funcs)
	for _, typ := range pkg.Types {
		spec := typ.Decl.Specs[0].(*ast.TypeSpec)
		methods := typ.Methods
		if target := internal[runtimeAlias(spec)]; target != nil {
			named := *target.Decl.Specs[0].(*ast.TypeSpec)
			named.Name = spec.Name
			spec, methods = &named, target.Methods
		}
		lines = append(lines, "type "+source(fset, stripType(spec)))
		values(typ.Consts)
		values(typ.Vars)
		funcs(typ.Funcs)
		funcs(methods)
	}
	return lines
}

// runtimeAlias is the name of the internal/runtime type `spec` is an
// alias of (`type X = runtime.Y` or `runtime.Y[C]`), else "".
func runtimeAlias(spec *ast.TypeSpec) string {
	if !spec.Assign.IsValid() {
		return ""
	}
	typ := spec.Type
	if index, ok := typ.(*ast.IndexExpr); ok {
		typ = index.X
	}
	if sel, ok := typ.(*ast.SelectorExpr); ok {
		if pkg, ok := sel.X.(*ast.Ident); ok && pkg.Name == "runtime" {
			return sel.Sel.Name
		}
	}
	return ""
}

// stripValue drops comments, and Version's value: the release train
// changes it every release, not the API.
func stripValue(spec *ast.ValueSpec) *ast.ValueSpec {
	out := &ast.ValueSpec{Names: spec.Names, Type: spec.Type, Values: spec.Values}
	if lit, ok := firstValue(spec).(*ast.BasicLit); ok && spec.Names[0].Name == "Version" && lit.Kind == token.STRING {
		out.Values, out.Type = nil, ast.NewIdent("string")
	}
	return out
}

func firstValue(spec *ast.ValueSpec) ast.Expr {
	if len(spec.Values) == 0 {
		return nil
	}
	return spec.Values[0]
}

// stripType keeps a struct's exported fields and an interface's methods,
// without comments.
func stripType(spec *ast.TypeSpec) *ast.TypeSpec {
	out := &ast.TypeSpec{Name: spec.Name, TypeParams: spec.TypeParams, Assign: spec.Assign, Type: spec.Type}
	switch typ := spec.Type.(type) {
	case *ast.StructType:
		fields := &ast.FieldList{}
		for _, field := range typ.Fields.List {
			var names []*ast.Ident
			for _, name := range field.Names {
				if name.IsExported() {
					names = append(names, name)
				}
			}
			if len(names) > 0 || (len(field.Names) == 0 && exportedEmbed(field.Type)) {
				fields.List = append(fields.List, &ast.Field{Names: names, Type: field.Type, Tag: field.Tag})
			}
		}
		out.Type = &ast.StructType{Fields: fields}
	case *ast.InterfaceType:
		methods := &ast.FieldList{}
		for _, field := range typ.Methods.List {
			methods.List = append(methods.List, &ast.Field{Names: field.Names, Type: field.Type})
		}
		out.Type = &ast.InterfaceType{Methods: methods}
	}
	return out
}

func exportedEmbed(expr ast.Expr) bool {
	switch e := expr.(type) {
	case *ast.StarExpr:
		return exportedEmbed(e.X)
	case *ast.SelectorExpr:
		return e.Sel.IsExported()
	case *ast.Ident:
		return e.IsExported()
	}
	return false
}

func source(fset *token.FileSet, node any) string {
	var buf bytes.Buffer
	if err := (&printer.Config{Mode: printer.UseSpaces, Tabwidth: 4}).Fprint(&buf, token.NewFileSet(), node); err != nil {
		panic(err)
	}
	return buf.String()
}

// diff is a plain line diff: lines only in the snapshot (-), only in the
// current API (+).
func diff(want, got string) string {
	in := func(lines []string) map[string]bool {
		set := map[string]bool{}
		for _, line := range lines {
			set[line] = true
		}
		return set
	}
	wantLines, gotLines := strings.Split(want, "\n"), strings.Split(got, "\n")
	wantSet, gotSet := in(wantLines), in(gotLines)
	var out strings.Builder
	out.WriteString("--- go/rutis/public-api.txt\n+++ current\n")
	for _, line := range wantLines {
		if !gotSet[line] {
			out.WriteString("-" + line + "\n")
		}
	}
	for _, line := range gotLines {
		if !wantSet[line] {
			out.WriteString("+" + line + "\n")
		}
	}
	return out.String()
}
