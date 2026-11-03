# Compatible Genesis dependency pins

Katagami curation must install from its full Genesis commit without conflicting versions in the dependency closure. Existing unpinned paw-agent and paw-research dependencies resolve stale registry versions that conflict with Katagami pins. Pin the existing canonical agent to the cleaned paw-fs revision, and research to that agent revision. Preserve all application code.

The installer must resolve exactly one version per owner/app. Verify the actual returned closure, then exercise the existing contribution path. No kernel or policy changes.
