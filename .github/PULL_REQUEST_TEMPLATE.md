## What this changes

<!-- One or two sentences. Link the issue it closes, if any. -->

## Why

<!-- What was wrong or missing before. -->

## How it was verified

<!-- Commands you ran and what you saw. Include the target URL if the change affects audit output. -->

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] Documented flags match the implemented flags
- [ ] Docs, skill descriptions, and README updated where relevant

## Notes for reviewers

- [ ] No credentials, tokens, or client data in the diff
- [ ] No new network endpoints, or they are documented in the README
- [ ] Behaviour without API keys is unchanged, or the change is called out above
