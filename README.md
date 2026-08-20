# Backup Converter

Converts backups from app A to app B.

### Supports
- Aidoku (read/ write)
- Tachiyomi (read)

### Quirks
Some sources may have certain quirks (see [`crates/aidoku/src/quirks.rs`](crates/aidoku/src/quirks.rs) for examples) in how their extensions save their data. I have remedied some of these quirks specifically on the Aidoku side. If one of your sources aren't converting correctly (ie. when Aidoku goes to refresh nothing shows or all chapter history gets reset) it most likely has a quirk that needs implementing.
