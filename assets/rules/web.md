## Requirements for Working with Web Projects
- Use the project's package manager (`npm`, `yarn`, `pnpm`, or `bun`) to add dependencies. Prefer the one already in use (check `lock` files). Never edit `package.json` by hand for dependency changes.
- Run `npx tsc --noEmit` (TypeScript) or the project's type checker to validate types when unsure about APIs or interfaces.
- Before completing your task, whenever you modify web code:
    1. Run the project's linter (e.g. `npm run lint` or `npx eslint .`) and fix all errors and warnings
    2. Run the project's formatter (e.g. `npm run format` or `npx prettier --write .`) to apply consistent styling
    3. If TypeScript is used, run `npx tsc --noEmit` and resolve all type errors

## CSS & Styling
- Use the project's existing styling approach (Tailwind, CSS Modules, styled-components, etc.). Do not mix styling paradigms without a clear reason.
- When using Tailwind CSS, prefer utility classes over custom CSS. Use `@apply` sparingly and only for truly reusable patterns.
- Ensure responsive design: test layouts at mobile, tablet, and desktop breakpoints.
