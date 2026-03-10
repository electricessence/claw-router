---
applyTo: "**"
---
# Phased Commit --- Mandatory for Every Commit

**1 Stage** --- relevant files only; no artifacts, temp files, or unintentional changes.

**2 Review** --- adversarial diff check: correct intent, no logic errors or side effects, tests intact, accurate commit message. Anything wrong: unstage, fix, restart.

**3 Security Audit WARNING** --- scan every staged file:

| Category | Examples |
|---|---|
| Keys / tokens | sk-ant-, sk-or-, Bearer, Authorization: values |
| Passwords / secrets | Any credential-looking string |
| Hostnames / IPs | Internal names, LAN IPs, private domains |
| SSH paths | Machine-specific key paths |
| PII | Names, emails, phone numbers, user IDs |
| Env var values | Actual secret values (var **names** are OK) |

Any hit: remove it and restart from 1 before committing.

**4 Commit** --- present-tense message (`Add X`, `Fix Y`, `Update Z`). Commit locally.

**5 Push - EXPLICIT APPROVAL REQUIRED** --- always get approval before `git push`; report commit + concerns and wait for confirmation.

**6 PR Quality Loop — MANDATORY** --- after every push to a PR branch:

1. **Trigger Copilot review** — request a fresh review immediately after pushing.
2. **Wait for comments** — Copilot typically responds within 5 minutes.
3. **Address every comment** — fix the issue or explain briefly (human-style, not long-winded) why no change is needed. Resolve each thread.
4. **Re-trigger Copilot review** — after addressing all comments, request another review.
5. **Repeat** until Copilot returns zero new comments.
6. **Only then merge** — a clean Copilot review with no new comments is the merge gate.
