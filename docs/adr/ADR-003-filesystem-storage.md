# ADR 003 · Immutable UUID files

Streaming transfers write to a private staging directory. Final files receive internal UUID names unrelated to user keys. A volume marker pairs the storage root and catalog. Restart checks require referenced files to exist. Offline recovery can move unused files into an operator-reviewable archive. The current flat object directory should be benchmarked before adopting a sharded layout for larger catalogs.
