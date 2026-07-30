---
name: resume-application
description: This skill should be used when the user asks to "投递简历", "申请岗位", "发送在线简历", "先聊再投", "apply to a job", "submit a resume", or "send an online resume".
version: 0.1.0
---

# Resume Application

Run a controlled, pure-command-line BossKit workflow for one AI Agent
opportunity. Treat conversation as qualification, not as permission to submit a
resume. Keep every platform write explicit, narrow, and verifiable.

## Boundaries

- Use BossKit commands only. Never open a browser or use browser automation.
- Restrict the target to an AI Agent role.
- Select one best role per company. Reject parallel applications to multiple
  roles at the same company.
- Never expose cookies, tokens, platform remote IDs, or personally identifiable
  information. Refer to a target by company, title, and local cached job ID.
- Never invent an endpoint, raw-call a platform API, or infer success from a
  local state change.

## Establish Readiness

1. Check local authentication state with:

   ```bash
   boss status --platform zhipin
   ```

2. Inspect the sanitized online resume snapshot with:

   ```bash
   boss account resume show
   ```

3. Find and inspect candidates with:

   ```bash
   boss search "AI Agent" --platform zhipin
   boss ls --platform zhipin
   boss show <LOCAL_JOB_ID>
   ```

4. Choose exactly one best role at a company. Record the company, title, local
   cached job ID, salary evidence, schedule evidence, and relevant skill fit.
5. Stop before contact when the stated monthly salary floor is below 10K, the
   role is not an AI Agent role, or a duplicate same-company conversation or
   application already exists.
6. Treat missing or ambiguous salary and double-weekend information as
   unresolved hard gates, never as implied acceptance.

## Start a Human Conversation

1. Begin with the platform greeting only after the target has passed initial
   inspection:

   ```bash
   boss chat greet <LOCAL_JOB_ID> --yes
   ```

2. Never submit a resume on first contact. Never attach a resume to the
   greeting.
3. Personalize the opening to the exact role and one relevant strength. Avoid
   batch-identical introductions, generic spam, and any pitch for an automation
   product or workflow.
4. Read the exact conversation before composing another message:

   ```bash
   boss chat history <LOCAL_JOB_ID> --limit 20
   boss chat inbox <LOCAL_JOB_ID>
   ```

5. Send at most one concrete question per message:

   ```bash
   boss chat send --message "<ONE_CONCRETE_QUESTION>" <LOCAL_JOB_ID> --yes
   ```

6. Wait for a response before asking the next question or expanding activity.
   Never scale to more roles while the selected company conversation remains
   unresolved.
7. Learn the following naturally across the conversation:
   - whether salary can meet the 10K minimum;
   - whether double weekends are explicitly supported;
   - welfare details;
   - young-team context and collaboration style;
   - meal and housing support;
   - development-equipment policy.
8. Ask about the equipment policy neutrally. Never demand or directly ask for a
   MacBook Pro.
9. Stop after rejection. Send no unsolicited follow-up. Ask once for a
   rejection reason only when explicitly requested.

## Enforce Submission Gates

Require all conditions before considering submission:

- Confirm an AI Agent role and one-company/one-role uniqueness.
- Confirm salary evidence at or above 10K.
- Confirm explicit double-weekend evidence.
- Collect sufficient conversational evidence for welfare and working-context
  assessment, while allowing unanswered non-hard preferences to remain noted.
- Confirm no rejection and no prior submission to the exact target.
- Refresh the sanitized resume preview with `boss account resume show`.
- Inspect current capability with:

  ```bash
  boss account resume --help
  ```

Current BossKit exposes only `boss account resume show`; it has no platform
resume-submission command. When no documented `boss account resume submit` or
equivalent appears in current help, **STOP** and report the missing capability.
Never fabricate the command, call a raw endpoint, or claim that submission
occurred.

If a supported submission command appears in a future version, first present
the exact company, title, local cached job ID, sanitized resume preview, and
collected gate evidence. Request explicit confirmation immediately before that
single submission. Never auto-submit or reuse earlier consent. Execute only the
documented command, then verify the platform write through a documented
post-write read command. Report failure or unverifiable state honestly.

## Hand Off Evidence

Return a compact record without secrets, remote IDs, or PII:

```text
resume_submitted: true | false
exact_target: <company> / <title> / <local cached job ID>
resume_preview: <sanitized snapshot identity and section counts>
user_confirmation: <immediate confirmation present | absent>
conversation_evidence: <safe summary of greeting, replies, and hard-gate answers>
salary_gate: pass | fail | unknown
double_weekend_gate: pass | fail | unknown
rejection_status: not_rejected | rejected | unknown
no_duplicate_proof: <same-company role and prior-submission checks>
submission_verification: verified | failed | unsupported
```

Set `resume_submitted: true` only after a supported platform write and
independent post-write verification both succeed. Otherwise keep it `false` and
state the exact blocker.
