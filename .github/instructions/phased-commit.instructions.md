---
applyTo: "**"
---
# Phased Commit Procedure — Mandatory for Every Commit

Every commit follows these phases **in order**. Do not skip, combine, or rush any phase.

---

## Phase 1 — Stage

Stage **only** the files directly relevant to the current task.

**Never stage:**
- Files you didn't intentionally change
- Build artifacts, temporary files, editor metadata
- Anything outside the scope of the current task

---

## Phase 2 — Critical Review

Put on the dev-manager hat. Review every staged diff with **adversarial skepticism**:

- Does the change do exactly what was intended — nothing more, nothing less?
- Are there logic errors, missing error handling, or unintended side effects?
- Does the change break any tests, invariants, or documented behaviour?
- Is the commit message accurate and complete?

If anything looks wrong: unstage it, fix it, restart from Phase 1.

---

## Phase 3 — Security Audit ⚠️ HIGHEST PRIORITY

This phase is non-negotiable. Scan **every staged file** for:

| Category | Examples |
|----------|---------|
| API keys / tokens | `sk-ant-`, `sk-or-`, `Bearer `, `Authorization:` values |
| Passwords / secrets | Any string that looks like a credential |
| Hostnames / IPs | Internal server names, LAN IPs, private domain names |
| SSH paths | Private key paths tied to a specific machine |
| Personal info | Real names, email addresses, phone numbers, user IDs |
| Env var values | Actual secret values (env var **names** are OK) |

**If ANY sensitive data is found: do NOT commit.** Remove it, return to Phase 1.

---

## Phase 4 — Commit

Once Phases 1–3 are clean:

- Write a clear, concise commit message in present tense (`Add X`, `Fix Y`, `Update Z`)
- Commit locally: `git commit -m "..."`

---

## Phase 5 — Push (REQUIRES EXPLICIT OPERATOR APPROVAL)

**Do NOT push without the operator explicitly saying to.**

After committing, report:
- What was committed and on which branch
- Any open concerns or follow-on work

Then **wait**. Do not run `git push` until the operator confirms.
