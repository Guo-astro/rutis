# Migrating to 0.18

[中文](migration-0.18.md)

0.18 makes failures understandable ([diagnostics design](design-diagnostics-2026-10-26.en.md)). This page grows with the changes of 0.18; below is what is done so far.

## Two entries on one listener accepting one token: an error at start (#702)

When a listening peer row and a runtime that connects in (`listen` + `instances` of `runtimes.remote`) would accept the same token on one listener (say neither has its own and both fall back to `RUTIS_TOKEN`), 0.17 started, but refused each connection with 403 (it could not tell which entry it was for), and the runtime retried every 30 seconds. From 0.18 `run`, `check` and a configuration reload fail before starting:

```
rutis-host: listener public: link row main and remote runtime phone both accept RUTIS_TOKEN, so a connection presenting it is refused as ambiguous: set RUTIS_TOKEN_MAIN or RUTIS_TOKEN_PHONE to a token of its own
```

Fix: as the error says, set `RUTIS_TOKEN_<name>` for one of them (and have the runtime that connects in present the new token). Such a configuration could not connect in 0.17 either; once fixed, it does.
