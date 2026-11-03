# ARN-512 — schema-level CSDL annotations survive the kernel

Foundry's Twins page lists a schema as a twin when `$metadata` shows
`<Annotation Term="Temper.Twin">` as a direct child of its `<Schema>`. The DSF
schema declares it; production `$metadata` never shows it, because the kernel's
CSDL model has no place for schema-level annotations: they are dropped on parse
and never emitted. Entity-level annotations survive.

Outcome: a schema-level annotation round-trips through parse, merge and emit,
so `$metadata` carries it and the Twins page lists "Deep Sci-Fi".
