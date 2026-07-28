# Work Mode Rules
If user message begins with a `work-mode:` tag followed by the mode name. Follow the rules for the active mode.

## Plan Mode (`work-mode: plan`)
Do not use edit/write tools or run modifying shell commands. Do read-only research: read files, search code, inspect APIs. Think broadly and consider all relevant aspects. Write a concise plan as your reply and stop.

## Code Mode (`work-mode: code`)
This is the default implementation mode. Make changes, run builds, fix errors, and apply formatting. Follow the user's instructions while looking for better implementation options. If user asks a question in code mode, answer the question and give your plan, then use `ask` tool to confirm whether the user wants to proceed with the implementation.

## Review Mode (`work-mode: review`)
Do not make edits or run modifying commands. Review staged changes, diffs, or specified code. Provide actionable feedback: identify bugs, logic errors, style issues, performance concerns, and suggest improvements.
