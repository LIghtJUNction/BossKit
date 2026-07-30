---
name: resume-screening
description: This skill should be used when the user asks to "筛选简历", "筛选匹配岗位", "评估简历与职位匹配", "简历匹配职位", "screen a resume", "screen matching jobs", or "evaluate resume-job fit".
version: 0.1.0
---

# Resume Screening

Screen a typed local candidate resume against locally cached jobs through the
BossKit command line. Produce reproducible local `manual_review` or `dry_run`
plans only.

## Boundaries

- Treat the workflow as candidate-side job screening.
- Never claim recruiter-side access to candidate pools, employer inboxes, or
  platform applicant data.
- Never greet, chat, apply, submit a resume, or perform any platform resume
  write.
- Never open a browser or use browser automation.
- Keep online-resume text ephemeral. Never persist, import, or copy it into a
  local resume automatically.
- Never expose cookies, tokens, platform remote IDs, or PII in evidence.
- Describe matching as deterministic title-and-skill screening. Never imply
  semantic understanding or recruiter-grade ranking.

## Select a Typed Resume

1. List and inspect local typed resumes:

   ```bash
   boss resume ls
   boss resume show <RESUME_NAME>
   ```

2. Compare known local revisions when necessary:

   ```bash
   boss resume diff <LEFT_NAME> <RIGHT_NAME>
   ```

3. Clone a revision only when a separate local screening variant is requested:

   ```bash
   boss resume clone <RESUME_NAME> <NEW_NAME>
   ```

4. Read the sanitized online snapshot only with:

   ```bash
   boss account resume show
   ```

5. Request explicit consent before persisting any online-resume-derived content.
   After consent, require a reviewed strict JSON file and import it explicitly:

   ```bash
   boss resume import <PATH>
   ```

   Never treat consent as proof that an automatic online-to-local conversion
   exists.

## Prepare Cached Jobs

1. Search for relevant AI Agent jobs and cache the results:

   ```bash
   boss search "AI Agent"
   ```

2. Enumerate and inspect exact cached records:

   ```bash
   boss ls
   boss show <LOCAL_JOB_ID>
   ```

3. Deduplicate by company and role. Retain at most one best role per company in
   the final review set.
4. Exclude known blacklisted companies, job descriptions, and jobs. Inspect
   configured local blacklist rules when needed:

   ```bash
   boss campaign blacklist ls
   ```

## Define Hard Gates

Create or inspect a reproducible local policy:

```bash
boss campaign policy ls
boss campaign policy show <POLICY_NAME>
boss campaign policy add <POLICY_NAME> --monthly-salary-min 10000
```

Add only verified include, exclude, welfare, salary, and score rules supported
by `boss campaign policy add --help`. Preserve the following decisions in the
policy or the manual-review evidence:

- Require an AI Agent title or direct role evidence.
- Require a monthly salary floor of at least 10K.
- Require explicit double-weekend evidence; treat silence or ambiguous schedule
  wording as unknown.
- Record welfare evidence without inferring absent benefits.
- Compare experience and education requirements against the selected resume.
- Apply location requirements and mismatches explicitly.
- Exclude blacklist matches.

Classify every unknown hard requirement as `manual_review`, never as pass.
Classify a verified hard-gate violation as excluded. Keep preferences separate
from hard gates.

## Screen and Review

1. Run deterministic local screening:

   ```bash
   boss campaign screen --resume <RESUME_NAME> --policy <POLICY_NAME>
   ```

2. Treat the generated ranking as a title-and-skill signal only. Inspect each
   retained cached job with `boss show <LOCAL_JOB_ID>` before accepting its
   evidence.
3. Review generated local plans:

   ```bash
   boss campaign plan ls
   ```

4. Summarize local workflow counts only after screening:

   ```bash
   boss campaign stats
   ```

5. Leave all results in `manual_review` or `dry_run`. Do not transition the
   workflow into greeting, chatting, application, or resume submission.

## Report Reproducible Evidence

Return a compact screening record:

```text
mode: candidate_side_local_screening
policy: <name and effective rules>
resume: <local name and revision identity>
cached_jobs: <local cached job IDs inspected>
scores: <deterministic scores and matched title/skills>
excluded: <job, gate, and supporting evidence>
unknowns: <job and unresolved hard requirements>
manual_review: <deduplicated retained jobs>
dedupe: <company/role duplicate decisions>
counts: <screened, excluded, unknown, retained>
```

Keep salary, schedule, welfare, experience, education, location, and blacklist
evidence traceable to the selected resume, policy, or exact cached job. Mark
missing evidence as unknown. Never report the deterministic matcher as proof of
overall suitability.
