//! A plugin whose declared or embedded identity differs from this host's is
//! refused before `dlopen` (#631): a library's initializers run as soon as it
//! is mapped, so every check that reads only files runs first.
//!
//! Each load goes through [`Loader::testing`], whose backend replaces the
//! open and the symbol lookup: a rejected plugin must leave the recorded
//! opens empty; a plugin that passes the file checks must reach the injected
//! open with the cache path. Nothing here maps a library.

use rutis_dylib::{Backend, LoadError, Loader};
use rutis_sdk::{BOOT_MAGIC, BOOT_SIZE, SDK_ID, SDK_RUSTC_VERSION, SDK_TARGET, SDK_VERSION};
use sha2::{Digest, Sha256};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

/// The artifact this host was built against: a manifest and a boot blob must
/// repeat it. Its value is the loader's; nothing here reads the SDK file.
const HOST_ARTIFACT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// The names the host's SDK and libstd copies have, as dependency checks see
/// them; a plugin must link exactly these.
const SDK_LIBRARY: &str = "librutis_sdk.so";
const STD_LIBRARY: &str = "libstd-0123.so";
/// The plugin's own library file.
const LIBRARY: &str = "libgreeter.so";

/// Replaces the platform calls a load makes once its file checks pass, and
/// records every path an open was asked for.
struct Recorder {
    opened: Arc<Mutex<Vec<PathBuf>>>,
    /// What the injected open returns; `None` refuses it, as `dlopen` does
    /// for a file that cannot be mapped.
    handle: Option<usize>,
}

impl Backend for Recorder {
    unsafe fn open(&self, path: &Path) -> Result<NonNull<c_void>, String> {
        self.opened.lock().unwrap().push(path.to_path_buf());
        match self.handle {
            Some(handle) => Ok(NonNull::new(handle as *mut c_void).unwrap()),
            None => Err("test: nothing was mapped".into()),
        }
    }

    unsafe fn symbol(&self, _handle: NonNull<c_void>, name: &[u8]) -> Result<*mut c_void, String> {
        let name = String::from_utf8_lossy(name.strip_suffix(&[0]).unwrap_or(name));
        Err(format!("test: {name} is not exported"))
    }
}

/// A loader whose host checks are skipped and whose opens are recorded;
/// `handle` is what an injected open returns, `None` refusing it.
fn loader_with(
    cache: &Path,
    interfaces: &[(&str, &str)],
    max_versions: usize,
    handle: Option<usize>,
) -> (Loader, Arc<Mutex<Vec<PathBuf>>>) {
    let opened = Arc::new(Mutex::new(Vec::new()));
    let loader = Loader::testing(
        HOST_ARTIFACT,
        SDK_LIBRARY,
        STD_LIBRARY,
        cache,
        interfaces,
        max_versions,
        Box::new(Recorder {
            opened: opened.clone(),
            handle,
        }),
    );
    (loader, opened)
}

/// The manifest the packager writes beside a plugin.
#[derive(Clone)]
struct Manifest {
    id: String,
    version: String,
    library: String,
    /// `None` writes the hash of the file that is actually written.
    library_sha256: Option<String>,
    native_deps: Vec<String>,
    sdk_version: String,
    sdk_id: String,
    sdk_artifact: String,
    target: String,
    rustc: String,
    lock_sha256: String,
    interfaces: Vec<(String, String)>,
}

impl Manifest {
    fn matching() -> Self {
        Self {
            id: "greeter".into(),
            version: "1.2.3".into(),
            library: LIBRARY.into(),
            library_sha256: None,
            native_deps: Vec::new(),
            sdk_version: SDK_VERSION.into(),
            sdk_id: SDK_ID.into(),
            sdk_artifact: HOST_ARTIFACT.into(),
            target: SDK_TARGET.into(),
            rustc: SDK_RUSTC_VERSION.into(),
            lock_sha256: "c".repeat(64),
            interfaces: Vec::new(),
        }
    }
}

/// The boot blob `export_plugin!` embeds in the library.
#[derive(Clone)]
struct Boot {
    magic: Vec<u8>,
    sdk_id: String,
    sdk_artifact: String,
    id: String,
    version: String,
    /// A nonzero byte after the fields: ignored by the parser, and enough to
    /// make another library file for the same identity.
    tail: u8,
}

impl Boot {
    fn matching() -> Self {
        Self {
            magic: BOOT_MAGIC.to_vec(),
            sdk_id: SDK_ID.into(),
            sdk_artifact: HOST_ARTIFACT.into(),
            id: "greeter".into(),
            version: "1.2.3".into(),
            tail: 0,
        }
    }

    fn blob(&self) -> Vec<u8> {
        let mut blob = self.magic.clone();
        for field in [&self.sdk_id, &self.sdk_artifact, &self.id, &self.version] {
            blob.extend_from_slice(&(field.len() as u16).to_le_bytes());
            blob.extend_from_slice(field.as_bytes());
        }
        blob.resize(BOOT_SIZE, 0);
        if self.tail != 0 {
            *blob.last_mut().unwrap() = self.tail;
        }
        blob
    }
}

/// A plugin directory: the manifest plus the library file it names.
#[derive(Clone)]
struct Plugin {
    manifest: Manifest,
    boot: Boot,
    /// The libraries the binary itself links, as its dynamic section records
    /// them. The manifest must declare exactly the native ones.
    needed: Vec<String>,
    /// The boot section the library carries, which is not the one the loader
    /// looks for when this differs.
    boot_section: Vec<u8>,
    /// Appends a byte after the manifest's hash was taken, as tampering with
    /// the library file does.
    tamper: bool,
}

impl Plugin {
    fn matching() -> Self {
        Self {
            manifest: Manifest::matching(),
            boot: Boot::matching(),
            needed: vec![SDK_LIBRARY.into(), STD_LIBRARY.into()],
            boot_section: b".note.rutis.meta".to_vec(),
            tamper: false,
        }
    }

    /// The library file: a plugin image built for this host, or bytes an
    /// object reader refuses.
    fn bytes(&self) -> Vec<u8> {
        plugin_object(&self.boot_section, &self.boot.blob(), &self.needed)
    }

    /// Writes the directory and returns the file's hash, as the manifest
    /// declares it unless the manifest names another one.
    fn write(&self, dir: &Path) -> String {
        let mut bytes = self.bytes();
        let hash = sha(&bytes);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("plugin.toml"), self.manifest_text(&hash)).unwrap();
        if self.tamper {
            bytes.push(0);
        }
        // The file is always `LIBRARY`; a manifest that names another one is
        // what some tests are about, and that file need not exist.
        std::fs::write(dir.join(LIBRARY), &bytes).unwrap();
        hash
    }

    fn manifest_text(&self, actual: &str) -> String {
        let m = &self.manifest;
        let declared = m.library_sha256.as_deref().unwrap_or(actual);
        let mut text = format!(
            "[plugin]\nid = {:?}\nversion = {:?}\nlibrary = {:?}\nlibrary_sha256 = {:?}\nnative_deps = {:?}\n\n[sdk]\nversion = {:?}\nid = {:?}\nartifact_sha256 = {:?}\n\n",
            m.id, m.version, m.library, declared, m.native_deps, m.sdk_version, m.sdk_id, m.sdk_artifact,
        );
        if !m.interfaces.is_empty() {
            text.push_str("[interfaces]\n");
            for (name, requirement) in &m.interfaces {
                text.push_str(&format!("{name:?} = {requirement:?}\n"));
            }
            text.push('\n');
        }
        text.push_str(&format!(
            "[build]\ntarget = {:?}\nrustc = {:?}\nlock_sha256 = {:?}\n",
            m.target, m.rustc, m.lock_sha256
        ));
        text
    }
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Loads `dir`, which must be refused before any open, and returns the error.
fn rejected(loader: &Loader, opened: &Arc<Mutex<Vec<PathBuf>>>, dir: &Path) -> LoadError {
    let error = match unsafe { loader.load(dir) } {
        Ok(_) => panic!("a mismatched plugin was accepted"),
        Err(error) => error,
    };
    let opened = opened.lock().unwrap();
    assert!(
        opened.is_empty(),
        "a rejected plugin reached an open: {opened:?}"
    );
    error
}

fn assert_reason(error: &LoadError, step: &str, reason: &str) {
    assert_eq!(error.step, step, "{error}");
    assert!(
        error.reason.contains(reason),
        "{error}: expected {reason:?}"
    );
}

// --- Checks that read only the manifest and the file hash -------------------

#[test]
fn a_manifest_for_another_platform_is_rejected_before_open() {
    let dir = tempdir();
    let cache = dir.path().join("cache");
    let mut plugin = Plugin::matching();
    plugin.manifest.target = if SDK_TARGET.contains("linux") {
        "aarch64-apple-darwin".into()
    } else {
        "x86_64-unknown-linux-gnu".into()
    };
    plugin.write(&dir.path().join("plugin"));
    let (loader, opened) = loader_with(&cache, &[], 4, None);
    let error = rejected(&loader, &opened, &dir.path().join("plugin"));
    assert_reason(&error, "manifest", "target mismatch");
}

#[test]
fn a_manifest_for_another_compiler_is_rejected_before_open() {
    let dir = tempdir();
    let cache = dir.path().join("cache");
    let mut plugin = Plugin::matching();
    plugin.manifest.rustc = "rustc 0.0.0 (000000000 1970-01-01)".into();
    plugin.write(&dir.path().join("plugin"));
    let (loader, opened) = loader_with(&cache, &[], 4, None);
    let error = rejected(&loader, &opened, &dir.path().join("plugin"));
    assert_reason(&error, "manifest", "rustc mismatch");
}

#[test]
fn a_manifest_for_another_sdk_is_rejected_before_open() {
    for change in [
        |m: &mut Manifest| m.sdk_id = "not-this-sdk".into(),
        |m: &mut Manifest| m.sdk_artifact = "b".repeat(64),
        |m: &mut Manifest| m.sdk_version = "0.0.0".into(),
    ] {
        let dir = tempdir();
        let cache = dir.path().join("cache");
        let mut plugin = Plugin::matching();
        change(&mut plugin.manifest);
        plugin.write(&dir.path().join("plugin"));
        let (loader, opened) = loader_with(&cache, &[], 4, None);
        let error = rejected(&loader, &opened, &dir.path().join("plugin"));
        assert_reason(&error, "manifest", "SDK");
    }
}

#[test]
fn a_manifest_field_that_is_not_a_hash_is_rejected_before_open() {
    for change in [
        |m: &mut Manifest| m.library_sha256 = Some("not a hash".into()),
        |m: &mut Manifest| m.lock_sha256 = "0123".into(),
    ] {
        let dir = tempdir();
        let cache = dir.path().join("cache");
        let mut plugin = Plugin::matching();
        change(&mut plugin.manifest);
        plugin.write(&dir.path().join("plugin"));
        let (loader, opened) = loader_with(&cache, &[], 4, None);
        let error = rejected(&loader, &opened, &dir.path().join("plugin"));
        assert_reason(&error, "manifest", "invalid SHA-256 field");
    }
}

#[test]
fn interfaces_that_are_not_met_are_rejected_before_open() {
    let dir = tempdir();
    let cache = dir.path().join("cache");
    for (requirement, host, reason) in [
        ("^1", &[][..], "missing interface"),
        ("^2", &[("rutis-iface-llm", "1.3.0")][..], "does not meet"),
    ] {
        let mut plugin = Plugin::matching();
        plugin.manifest.interfaces = vec![("rutis-iface-llm".to_string(), requirement.to_string())];
        plugin.write(&dir.path().join("plugin"));
        let (loader, opened) = loader_with(&cache, host, 4, None);
        let error = rejected(&loader, &opened, &dir.path().join("plugin"));
        assert_reason(&error, "interfaces", reason);
    }
}

#[test]
fn a_library_name_with_a_path_is_rejected_before_open() {
    for library in ["../libgreeter.so", "sub/libgreeter.so", ""] {
        let dir = tempdir();
        let cache = dir.path().join("cache");
        let mut plugin = Plugin::matching();
        plugin.manifest.library = library.into();
        plugin.write(&dir.path().join("plugin"));
        let (loader, opened) = loader_with(&cache, &[], 4, None);
        let error = rejected(&loader, &opened, &dir.path().join("plugin"));
        assert_reason(&error, "manifest", "library");
    }
}

#[test]
fn a_library_whose_hash_differs_from_the_manifest_is_rejected_before_open() {
    let dir = tempdir();
    let cache = dir.path().join("cache");
    let mut plugin = Plugin::matching();
    plugin.manifest.library_sha256 = Some("d".repeat(64));
    plugin.write(&dir.path().join("plugin"));
    let (loader, opened) = loader_with(&cache, &[], 4, None);
    let error = rejected(&loader, &opened, &dir.path().join("plugin"));
    assert_reason(&error, "binary", "library SHA-256 mismatch");
}

#[test]
fn bytes_that_are_not_an_object_file_are_rejected_before_open() {
    let dir = tempdir();
    let cache = dir.path().join("cache");
    std::fs::create_dir_all(dir.path().join("plugin")).unwrap();
    std::fs::write(
        dir.path().join("plugin").join(LIBRARY),
        b"not an object file",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("plugin").join("plugin.toml"),
        Plugin::matching().manifest_text(&sha(b"not an object file")),
    )
    .unwrap();
    let (loader, opened) = loader_with(&cache, &[], 4, None);
    let error = rejected(&loader, &opened, &dir.path().join("plugin"));
    assert_reason(&error, "boot metadata", "not an object file");
}

#[test]
fn an_object_for_another_format_is_rejected_before_open() {
    let format = if SDK_TARGET.contains("apple") {
        object::BinaryFormat::Elf
    } else {
        object::BinaryFormat::MachO
    };
    let bytes = object::write::Object::new(format, host_architecture(), object::Endianness::Little)
        .write()
        .unwrap();
    let dir = tempdir();
    let cache = dir.path().join("cache");
    let error = load_bytes(&dir, &cache, &bytes);
    assert_reason(&error, "boot metadata", &format!("found {format:?}"));
}

#[test]
fn an_object_for_another_architecture_is_rejected_before_open() {
    // PE images are written by hand: `object` writes only objects, not
    // images, and a COFF object would be refused for its format first.
    let bytes = if cfg!(windows) {
        pe_image(other_machine())
    } else {
        let other = match host_architecture() {
            object::Architecture::X86_64 => object::Architecture::Aarch64,
            _ => object::Architecture::X86_64,
        };
        let format = if SDK_TARGET.contains("apple") {
            object::BinaryFormat::MachO
        } else {
            object::BinaryFormat::Elf
        };
        object::write::Object::new(format, other, object::Endianness::Little)
            .write()
            .unwrap()
    };
    let dir = tempdir();
    let cache = dir.path().join("cache");
    let error = load_bytes(&dir, &cache, &bytes);
    assert_reason(&error, "boot metadata", "architecture mismatch");
}

fn other_machine() -> object::pe::Machine {
    if cfg!(target_arch = "x86_64") {
        object::pe::IMAGE_FILE_MACHINE_ARM64
    } else {
        object::pe::IMAGE_FILE_MACHINE_AMD64
    }
}

/// A PE32+ image for `machine`, with no sections: the loader reads the
/// header before anything else.
fn pe_image(machine: object::pe::Machine) -> Vec<u8> {
    use object::pe;
    let mut bytes = Vec::new();
    let mut w = object::write::pe::Writer::new(true, 0x1000, 0x200, &mut bytes);
    w.reserve_dos_header_and_stub();
    w.reserve_nt_headers(pe::IMAGE_NUMBEROF_DIRECTORY_ENTRIES);
    w.reserve_section_headers(0);
    w.write_dos_header_and_stub().unwrap();
    w.write_nt_headers(object::write::pe::NtHeaders {
        machine,
        time_date_stamp: 0,
        characteristics: pe::IMAGE_FILE_EXECUTABLE_IMAGE | pe::IMAGE_FILE_DLL,
        major_linker_version: 14,
        minor_linker_version: 0,
        address_of_entry_point: 0,
        image_base: 0x1_8000_0000,
        major_operating_system_version: 6,
        minor_operating_system_version: 0,
        major_image_version: 0,
        minor_image_version: 0,
        major_subsystem_version: 6,
        minor_subsystem_version: 0,
        subsystem: pe::IMAGE_SUBSYSTEM_WINDOWS_CUI,
        dll_characteristics: pe::DllFlags(0),
        size_of_stack_reserve: 0x10_0000,
        size_of_stack_commit: 0x1000,
        size_of_heap_reserve: 0x10_0000,
        size_of_heap_commit: 0x1000,
    });
    w.write_section_headers();
    bytes
}

/// Writes `bytes` behind a matching manifest and returns what a load of it
/// rejects before the open.
fn load_bytes(dir: &tempfile::TempDir, cache: &Path, bytes: &[u8]) -> LoadError {
    let mut plugin = Plugin::matching();
    plugin.manifest.library_sha256 = Some(sha(bytes));
    std::fs::create_dir_all(dir.path().join("plugin")).unwrap();
    std::fs::write(dir.path().join("plugin").join(LIBRARY), bytes).unwrap();
    std::fs::write(
        dir.path().join("plugin").join("plugin.toml"),
        plugin.manifest_text(&sha(bytes)),
    )
    .unwrap();
    let (loader, opened) = loader_with(cache, &[], 4, None);
    rejected(&loader, &opened, &dir.path().join("plugin"))
}

fn host_architecture() -> object::Architecture {
    match std::env::consts::ARCH {
        "x86_64" => object::Architecture::X86_64,
        "aarch64" => object::Architecture::Aarch64,
        other => panic!("no fixture for {other}"),
    }
}

/// The library file of a plugin for this host. ELF is written here; the other
/// formats are covered by `tools/test-dylib.sh` with real plugin artifacts.
fn plugin_object(boot_section: &[u8], boot: &[u8], needed: &[String]) -> Vec<u8> {
    #[cfg(target_os = "linux")]
    {
        elf::shared_object(host_machine(), boot_section, boot, needed)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (boot_section, boot, needed);
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn host_machine() -> u16 {
    match std::env::consts::ARCH {
        "x86_64" => object::elf::EM_X86_64.0,
        "aarch64" => object::elf::EM_AARCH64.0,
        other => panic!("no fixture for {other}"),
    }
}

#[cfg(target_os = "linux")]
mod elf {
    //! A minimal ELF64 shared object the loader can inspect: `.dynstr` and
    //! `.dynamic` hold its dependencies, `.note.rutis.meta` the boot blob,
    //! `.shstrtab` the names. It has no program headers and nothing to run.

    use object::elf;

    const HEADER: usize = 64;
    const SECTION: usize = 64;

    pub(super) fn shared_object(
        machine: u16,
        boot_section: &[u8],
        boot: &[u8],
        needed: &[String],
    ) -> Vec<u8> {
        let mut dynstr = vec![0_u8];
        let mut offsets = Vec::new();
        for name in needed {
            offsets.push(dynstr.len() as u64);
            dynstr.extend_from_slice(name.as_bytes());
            dynstr.push(0);
        }
        let mut dynamic = Vec::new();
        for offset in offsets {
            dynamic.extend_from_slice(&(elf::DT_NEEDED.0 as u64).to_le_bytes());
            dynamic.extend_from_slice(&offset.to_le_bytes());
        }
        dynamic.extend_from_slice(&(elf::DT_NULL.0 as u64).to_le_bytes());
        dynamic.extend_from_slice(&0_u64.to_le_bytes());

        let names: [&[u8]; 4] = [b".dynstr", b".dynamic", boot_section, b".shstrtab"];
        let mut shstrtab = vec![0_u8];
        let mut name_offsets = Vec::new();
        for name in names {
            name_offsets.push(shstrtab.len() as u32);
            shstrtab.extend_from_slice(name);
            shstrtab.push(0);
        }

        let dynstr_offset = HEADER;
        let dynamic_offset = dynstr_offset + dynstr.len();
        let boot_offset = dynamic_offset + dynamic.len();
        let shstrtab_offset = boot_offset + boot.len();
        let section_offset = (shstrtab_offset + shstrtab.len() + 7) & !7;
        let mut out = vec![0_u8; section_offset + SECTION * 5];
        let mut put = |at: usize, bytes: &[u8]| out[at..at + bytes.len()].copy_from_slice(bytes);

        put(0, b"\x7fELF");
        put(4, &[2, 1, 1]); // 64-bit, little endian, current version
        put(16, &(elf::ET_DYN.0).to_le_bytes());
        put(18, &machine.to_le_bytes());
        put(20, &1_u32.to_le_bytes());
        put(40, &(section_offset as u64).to_le_bytes());
        put(52, &(HEADER as u16).to_le_bytes());
        put(58, &(SECTION as u16).to_le_bytes());
        put(60, &5_u16.to_le_bytes());
        put(62, &4_u16.to_le_bytes());
        put(dynstr_offset, &dynstr);
        put(dynamic_offset, &dynamic);
        put(boot_offset, boot);
        put(shstrtab_offset, &shstrtab);

        let mut header = |index: usize,
                          name: u32,
                          kind: u32,
                          offset: usize,
                          size: usize,
                          link: u32,
                          align: u64,
                          entry: u64| {
            let at = section_offset + SECTION * index;
            put(at, &name.to_le_bytes());
            put(at + 4, &kind.to_le_bytes());
            put(at + 24, &(offset as u64).to_le_bytes());
            put(at + 32, &(size as u64).to_le_bytes());
            put(at + 40, &link.to_le_bytes());
            put(at + 48, &align.to_le_bytes());
            put(at + 56, &entry.to_le_bytes());
        };
        header(
            1,
            name_offsets[0],
            elf::SHT_STRTAB.0,
            dynstr_offset,
            dynstr.len(),
            0,
            1,
            0,
        );
        header(
            2,
            name_offsets[1],
            elf::SHT_DYNAMIC.0,
            dynamic_offset,
            dynamic.len(),
            1,
            8,
            16,
        );
        header(
            3,
            name_offsets[2],
            elf::SHT_PROGBITS.0,
            boot_offset,
            boot.len(),
            0,
            1,
            0,
        );
        header(
            4,
            name_offsets[3],
            elf::SHT_STRTAB.0,
            shstrtab_offset,
            shstrtab.len(),
            0,
            1,
            0,
        );
        out
    }
}

// --- A plugin the loader can read whole, and what it does after -----------

#[cfg(target_os = "linux")]
mod after_the_file_checks {
    use super::*;

    /// The address the injected open returns; nothing is ever called on it.
    const MAPPED: usize = 0x5eed;

    #[test]
    fn a_matching_plugin_reaches_the_injected_open() {
        let dir = tempdir();
        let cache = dir.path().join("cache");
        let plugin = Plugin::matching();
        let hash = plugin.write(&dir.path().join("plugin"));
        let (loader, opened) = loader_with(&cache, &[], 4, None);
        let error = match unsafe { loader.load(dir.path().join("plugin")) } {
            Ok(_) => panic!("an injected open that refuses was recorded as a success"),
            Err(error) => error,
        };
        assert_reason(&error, "dlopen", "test: nothing was mapped");
        let opened = opened.lock().unwrap();
        assert_eq!(
            *opened,
            [cache.join(&hash).join(LIBRARY)],
            "the open must be asked for the cache copy"
        );
    }

    #[test]
    fn a_binary_whose_identity_differs_from_the_manifest_is_rejected_before_open() {
        for change in [
            (|p: &mut Plugin| p.boot.sdk_id = "not-this-sdk".into()) as fn(&mut Plugin),
            |p: &mut Plugin| p.boot.sdk_artifact = "e".repeat(64),
            |p: &mut Plugin| p.boot.id = "other".into(),
            |p: &mut Plugin| p.boot.version = "9.9.9".into(),
            |p: &mut Plugin| p.manifest.id = "other".into(),
            |p: &mut Plugin| p.manifest.version = "9.9.9".into(),
        ] {
            let dir = tempdir();
            let cache = dir.path().join("cache");
            let mut plugin = Plugin::matching();
            change(&mut plugin);
            plugin.write(&dir.path().join("plugin"));
            let (loader, opened) = loader_with(&cache, &[], 4, None);
            let error = rejected(&loader, &opened, &dir.path().join("plugin"));
            assert_reason(
                &error,
                "boot metadata",
                "binary identity differs from manifest or host",
            );
        }
    }

    #[test]
    fn a_boot_blob_of_another_format_version_is_rejected_before_open() {
        let dir = tempdir();
        let cache = dir.path().join("cache");
        let mut plugin = Plugin::matching();
        plugin.boot.magic = b"RUTIS_PLUGIN_BOOT_V2\0".to_vec();
        plugin.write(&dir.path().join("plugin"));
        let (loader, opened) = loader_with(&cache, &[], 4, None);
        let error = rejected(&loader, &opened, &dir.path().join("plugin"));
        assert_reason(&error, "boot metadata", "invalid plugin boot section");
    }

    #[test]
    fn a_library_without_a_boot_section_is_rejected_before_open() {
        let dir = tempdir();
        let cache = dir.path().join("cache");
        let mut plugin = Plugin::matching();
        plugin.boot_section = b".note.other".to_vec();
        plugin.write(&dir.path().join("plugin"));
        let (loader, opened) = loader_with(&cache, &[], 4, None);
        let error = rejected(&loader, &opened, &dir.path().join("plugin"));
        assert_reason(&error, "boot metadata", "plugin boot section not found");
    }

    #[test]
    fn dependencies_that_differ_from_the_manifest_are_rejected_before_open() {
        for change in [
            (|p: &mut Plugin| p.needed.push("libz.so.1".into())) as fn(&mut Plugin),
            |p: &mut Plugin| p.manifest.native_deps = vec!["libz.so.1".into()],
        ] {
            let dir = tempdir();
            let cache = dir.path().join("cache");
            let mut plugin = Plugin::matching();
            change(&mut plugin);
            plugin.write(&dir.path().join("plugin"));
            let (loader, opened) = loader_with(&cache, &[], 4, None);
            let error = rejected(&loader, &opened, &dir.path().join("plugin"));
            assert_reason(&error, "dependencies", "binary links native libraries");
        }
    }

    #[test]
    fn missing_exports_are_rejected_after_the_open() {
        // The only mismatch a file cannot show: a library that is linked as
        // this host's plugin but does not export its entry.
        let dir = tempdir();
        let cache = dir.path().join("cache");
        Plugin::matching().write(&dir.path().join("plugin"));
        let (loader, opened) = loader_with(&cache, &[], 4, Some(MAPPED));
        let error = match unsafe { loader.load(dir.path().join("plugin")) } {
            Ok(_) => panic!("a library without exports was accepted"),
            Err(error) => error,
        };
        assert_reason(&error, "metadata", "rutis_plugin_meta is not exported");
        assert_eq!(opened.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_retry_reuses_the_mapping_and_the_version_limit_is_before_open() {
        let dir = tempdir();
        let cache = dir.path().join("cache");
        Plugin::matching().write(&dir.path().join("first"));
        let (loader, opened) = loader_with(&cache, &[], 1, Some(MAPPED));
        for _ in 0..2 {
            let error = match unsafe { loader.load(dir.path().join("first")) } {
                Ok(_) => panic!("a library without exports was accepted"),
                Err(error) => error,
            };
            assert_eq!(error.step, "metadata", "{error}");
        }
        assert_eq!(
            opened.lock().unwrap().len(),
            1,
            "a retry must reuse the retained mapping instead of opening again"
        );
        // Another build of the same plugin: another file, the same identity.
        let mut other = Plugin::matching();
        other.boot.tail = 1;
        other.write(&dir.path().join("second"));
        let before = opened.lock().unwrap().len();
        let error = match unsafe { loader.load(dir.path().join("second")) } {
            Ok(_) => panic!("a second version was accepted beyond the limit"),
            Err(error) => error,
        };
        assert_reason(&error, "retention", "version limit 1 reached");
        assert_eq!(
            opened.lock().unwrap().len(),
            before,
            "the version limit must stop the load before an open"
        );
    }

    /// A field a mismatched plugin can differ in. Each is always set to a
    /// value the host or the manifest cannot accept.
    #[derive(Clone, Copy, Debug)]
    enum Difference {
        Target,
        Rustc,
        SdkId,
        SdkArtifact,
        SdkVersion,
        Id,
        Version,
        LibraryHash,
        NativeDeps,
        Interface,
        BootSdkId,
        BootArtifact,
        BootId,
        BootVersion,
        BootMagic,
        Tamper,
    }

    const DIFFERENCES: [Difference; 16] = [
        Difference::Target,
        Difference::Rustc,
        Difference::SdkId,
        Difference::SdkArtifact,
        Difference::SdkVersion,
        Difference::Id,
        Difference::Version,
        Difference::LibraryHash,
        Difference::NativeDeps,
        Difference::Interface,
        Difference::BootSdkId,
        Difference::BootArtifact,
        Difference::BootId,
        Difference::BootVersion,
        Difference::BootMagic,
        Difference::Tamper,
    ];

    fn differ(plugin: &mut Plugin, difference: Difference, noise: u64) {
        let hex = |noise: u64| format!("{noise:016x}").repeat(4);
        let other_id = format!("other-{noise:x}");
        match difference {
            Difference::Target => {
                plugin.manifest.target =
                    ["aarch64-apple-darwin", "x86_64-pc-windows-msvc"][(noise % 2) as usize].into();
            }
            Difference::Rustc => plugin.manifest.rustc = format!("rustc 0.0.{}", noise % 10),
            Difference::SdkId => plugin.manifest.sdk_id = other_id,
            Difference::SdkArtifact => plugin.manifest.sdk_artifact = hex(noise),
            Difference::SdkVersion => plugin.manifest.sdk_version = "9.9.9".into(),
            Difference::Id => plugin.manifest.id = other_id,
            Difference::Version => plugin.manifest.version = format!("9.9.{noise}"),
            Difference::LibraryHash => plugin.manifest.library_sha256 = Some(hex(noise)),
            Difference::NativeDeps => plugin.manifest.native_deps = vec!["libz.so.1".into()],
            Difference::Interface => {
                plugin.manifest.interfaces = vec![("rutis-iface-llm".into(), "^1".into())]
            }
            Difference::BootSdkId => plugin.boot.sdk_id = other_id,
            Difference::BootArtifact => plugin.boot.sdk_artifact = hex(noise),
            Difference::BootId => plugin.boot.id = other_id,
            Difference::BootVersion => plugin.boot.version = format!("9.9.{noise}"),
            Difference::BootMagic => plugin.boot.magic = b"RUTIS_PLUGIN_BOOT_V2\0".to_vec(),
            Difference::Tamper => plugin.tamper = true,
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig {
            cases: 256,
            // The seed is printed with a failure; a saved file would be
            // another fixture to keep.
            failure_persistence: None,
            ..proptest::prelude::ProptestConfig::default()
        })]

        /// Any difference from what this host accepts is refused before the
        /// open, whichever fields differ and in whatever combination.
        #[test]
        fn any_difference_is_rejected_before_open(
            chosen in proptest::sample::subsequence(DIFFERENCES.to_vec(), 1..=4),
            noise in proptest::prelude::any::<u64>(),
        ) {
            let dir = tempdir();
            let cache = dir.path().join("cache");
            let mut plugin = Plugin::matching();
            for (index, difference) in chosen.iter().enumerate() {
                differ(&mut plugin, *difference, noise.wrapping_add(index as u64));
            }
            plugin.write(&dir.path().join("plugin"));
            let (loader, opened) = loader_with(&cache, &[], 4, None);
            let error = match unsafe { loader.load(dir.path().join("plugin")) } {
                Ok(_) => panic!("a mismatched plugin ({chosen:?}) was accepted"),
                Err(error) => error,
            };
            proptest::prop_assert!(
                opened.lock().unwrap().is_empty(),
                "a mismatched plugin ({chosen:?}) reached an open",
            );
            proptest::prop_assert!(
                ["manifest", "interfaces", "binary", "boot metadata", "dependencies"]
                    .contains(&error.step),
                "a mismatched plugin ({chosen:?}) failed at {}: {error}",
                error.step,
            );
        }
    }
}
