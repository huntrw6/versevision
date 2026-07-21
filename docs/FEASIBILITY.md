# Milestone 0 feasibility report

Status: **Windows spike passed for ProPresenter 19.0.1; macOS pending**

On July 18, 2026, the Windows feasibility spike succeeded against ProPresenter 19.0.1 (build 318767361), API v1, document platform version 10/build 26200. The configured Network API port was 1025. The API resolved the disposable `Verses` presentation to its UUID and filesystem path. The presentation decoded and deterministic re-encoding produced an identical 2,179-byte file and identical SHA-256 hash.

A blank styled cue was cloned offline, all non-nil nested object UUIDs were refreshed, synthetic text was injected into the cloned RTF element, and a new cue group was appended. The writer created a timestamped backup, wrote and flushed a same-directory temporary file, reparsed and verified it, and atomically replaced the target while ProPresenter was closed. ProPresenter reopened the file and visibly rendered the appended `VerseBridge Test` slide. `GET /v1/presentation/{uuid}` independently returned the new group and exact synthetic text.

The reverse-engineered MIT-licensed schema snapshot is from `greyshirtguy/ProPresenter7-Proto`, `Proto 19beta`, upstream commit `1b63dda196eb7e079721a8a4a7e7773520cb5ad2`. That snapshot reports build 318767123; compatibility remains explicitly gated to the locally verified 19.0.1/document 10/26200 combination.

Remaining before general production support:

- Equivalent macOS feasibility and application acceptance results.
- Named-object template validation with separate body/reference/version/copyright elements.
- Golden fixtures covering every claimed ProPresenter document version.
- Verification-failure rollback fault injection beyond backup restoration.
- Confirmed current OpenAPI playlist mutation routes and preservation tests.
- Sanitized golden fixtures and their supported document-version matrix.

## Open-application write observation

On July 18, 2026, a guarded atomic replacement was tested while ProPresenter 19.0.1 remained open. The replacement file parsed correctly, but ProPresenter retained its stale in-memory presentation. Reopening the target did not hot-reload the 73-cue replacement: ProPresenter preserved the 44-cue in-memory copy and created duplicate `Verses.pro.pro` files with new internal UUIDs. The same import succeeded with ProPresenter fully closed, then reopened with all 73 cues, 29 generated slides, and the two-slide continuation visible and API-verifiable. The exact original backup was subsequently restored and its SHA-256 hash verified. Production writes therefore require the ProPresenter process to be fully closed; switching away from the target is insufficient.

Unsupported document versions result in “No presentation files changed.” Direct writing remains an unofficial integration.
