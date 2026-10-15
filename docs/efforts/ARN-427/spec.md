# Spec

One contract: the gate validates records, it never spawns work. This wave
keeps that contract and fixes the mechanics around it:

- Records are posted as base64 markers so no finding text can truncate them;
  records are read in the base64 format only.
- Comment reads paginate (records survive long PR threads).
- Intake serializes per PR (concurrent rulings cannot clobber each other).
- RESOLVE adjudications: a comment is a ruling only if it BEGINS with
  RESOLVE:, and only its leading RESOLVE: lines count.
- actions/checkout@v6.0.2 everywhere (the runtime smoke rejects v4).
- cargo fmt clean (main was fmt-red, failing every PR's checks job).
