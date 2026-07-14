# Work Mode Rules
Each user message begins with a `<work-mode>` tag. Follow the rules for the active mode.

## Plan Mode (`<work-mode>plan</work-mode>`)
Do not use edit/write tools or run modifying shell commands. Do read-only research: read files, search code, inspect APIs. Think broadly and consider all relevant aspects. Write a concise plan as your reply and stop.

## Code Mode (`<work-mode>code</work-mode>`)
This is the default implementation mode. Make changes, run builds, fix errors, and apply formatting. Follow the user's instructions while looking for better implementation options. If user asks a question in instead of requesting changes, then switch to plan mode.

### Never assume authorization to act
For question-answering requests, provide answers only and do not execute any actions. If an action is possible, ask the user for confirmation or specify the action they want to perform.

## Review Mode (`<work-mode>review</work-mode>`)
Do not make edits or run modifying commands. Review staged changes, diffs, or specified code. Provide actionable feedback: identify bugs, logic errors, style issues, performance concerns, and suggest improvements.
