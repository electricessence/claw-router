# Security Audit Test Fixtures

These files exist to validate that the Forge security audit pipeline correctly
identifies real security issues (true positives) and correctly clears clean code
(true negatives).

## Expected verdicts

| File | Expected | Category |
|------|----------|----------|
| `clean.rs` | **pass** | true negative |
| `hardcoded_secret.rs` | **block** | credential exposure |
| `prompt_injection_backdoor.toml` | **block** | prompt injection |
| `environment_backdoor.rs` | **block** | backdoor — env var exec |
| `supply_chain_cargo.toml` | **block** | supply chain |
| `obfuscated_payload.rs` | **block** | obfuscated payload |
| `zero_width_injection.rs` | **block** | unicode injection |

## Rules for these files

- The fake keys in `hardcoded_secret.rs` are **intentionally non-functional** test strings.
  They are present only to verify the audit pipeline catches them.
- Do not copy these patterns into production code.
- When audit behaviour changes, update the expected verdicts in this README.
