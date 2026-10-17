# Embedded model catalog

`models-dev-snapshot.json.gz` is a field-projected, gzipped copy of
[models.dev](https://models.dev)'s `api.json`, embedded in the binary by
`luxe_providers::catalog::snapshot`. It is the offline floor for model metadata: a fresh install
with no network, no cache, and no config still resolves real context windows, output caps,
reasoning levels, and pricing.

Regenerate with `scripts/update-models-snapshot.sh` (CI does it weekly and opens a PR). The
script emits deterministic bytes, so an unchanged catalog produces no diff.

Source data is from [anomalyco/models.dev](https://github.com/anomalyco/models.dev), MIT licensed.
