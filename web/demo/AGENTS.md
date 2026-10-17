# todo

A task list. No dependencies, no build step — plain ES modules on Node.

```bash
npm test                              # node --test
node src/cli.js                       # the CLI, on ./tasks.json
TODO_FILE=/tmp/try.json node src/cli.js add 'x' --due 2026-03-04   # try it without touching that
```

- Dates are ISO day strings (`2026-03-14`) and compare as text. Never parse them into `Date`.
- Every filter in `src/filter.js` is pure and takes the task list first.
- A behavior change needs a test in `test/` that fails without it.
