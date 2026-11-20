# ADR 006 · Publish bytes before committing metadata

Each upload uses a fresh staging file. The server flushes the file to disk, atomically renames it, and synchronizes the affected directories on Linux. The database transaction then publishes the verified size, hashes and physical UUID. A failed or ambiguous transaction can strand a file. Recovery consults the catalog before moving unreferenced data. The old file survives concurrent reads and a failed overwrite. Filesystem and PostgreSQL backups must describe the same logical point in time.
