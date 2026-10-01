# Simulated calibration mechanics

This synthetic dataset demonstrates computation only. It is not historical evidence and cannot support a claim about real prediction quality.

1. Create a World, then OpenReplay with learning_mode=simulated, domain="Synthetic calibration demonstration", frontier_date="2026-01-01T00:00:00Z", last_ingest_date="2025-03-01T00:00:00Z".
2. Create an EventNode in that world with statement="Synthetic next event occurs", probability="0.55", provenance="authored", source_refs='["fixture:foresight-calibration-mechanics-v1"]', resolve_by="2025-06-01T00:00:00Z", layer="fast".
3. World.RegisterForecasts with last_ingest_date="2025-03-01T00:00:00Z" records the identity prediction.
4. Create LearningRun {}, then Start with world_id, mode="simulated", as_of="2025-03-01T00:00:00Z", dataset_json set to this fixture's JSON text.
5. Wait for Adopted. Inspect the actual report and fitted model. World.RegisterForecasts with last_ingest_date="2025-03-02T00:00:00Z" creates a revision applying that model. Repeating it creates no duplicate.
6. The latest Forecast.RecordOutcome accepts outcome="yes", outcome_source_refs='["fixture:foresight-calibration-mechanics-v1"]', resolved_at="2025-06-01T00:00:00Z". This scores the revision chain and creates a new learning run. One resolved underlying event is insufficient for another update, which must be reported as rejected.

The 36 examples have 24 temporally earlier training events and 12 later validation events. No output probabilities or fitted coefficients are stored in the fixture.
