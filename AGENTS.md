# Agent workflow

- After every push, wait for the GitHub Actions workflow for the pushed commit to finish.
- Do not report the work as complete while required CI jobs are pending or failing.
- If CI fails, inspect the failed job and step logs, reproduce the failure locally when possible, fix the cause, and push a follow-up commit.
- Re-check the new workflow run after each fix and continue until the required quality, PWA, deployment, and relevant platform jobs pass.
- Treat a skipped job, an ignored test, and a successful test as different states; explain any intentional skips or opt-in network tests explicitly.
