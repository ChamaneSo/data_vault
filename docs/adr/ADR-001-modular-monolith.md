# ADR 001 · One server process

The first deployment has one object volume and one catalog. A Cargo workspace shares domain rules with a Rust server whose modules cover HTTP, authorization, persistence and observability. A separate CLI communicates over HTTP. The storage port is asynchronous so a future backend can stream the same logical objects. Running a second server against the same catalog is refused by an ownership lock.
