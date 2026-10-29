# {{name}}

A [rutis](https://github.com/arcships/rutis) plugin. Guide: [Write a Python plugin](https://github.com/arcships/rutis/blob/main/docs/guide/python-plugin.en.md).

```bash
uv sync
uv run python -m unittest discover -s tests   # unit tests, no host needed
uv run rutis-host dev                         # run it in a local host; changes reload it
```

Publish with `uv build && uv publish` (the workflow in `.github/workflows/publish.yml` does it for a `v*` tag). A host installs it where its Python runtime runs and adds a row `{ "id": "{{id}}", "name": "py:{{id}}" }`.
