# ratelint

A linter for rate-limit rule files. It reads a small config format describing
per-route limits and reports problems with file and line numbers, the way a
compiler would.

## Why

Rate limit config tends to rot quietly. Someone adds a new route and forgets
the limit block, a copy-pasted rule keeps the old path, a "burst" value gets
left off so the limiter falls back to whatever the code defaults to, or a
limit gets typed as `500000` instead of `5000` and nobody notices until
traffic actually hits it. None of that fails a build. ratelint turns those
into lint errors that fail CI instead.

## The rule file format

A rules file is a flat, INI-like format. Each `[section]` is one rule:

```ini
# api-gateway.rules
[login]
path = /api/login
limit = 5
window = 60
burst = 10

[search]
path = /api/search
limit = 100
window = 60
```

Fields:

- `path` — the route this rule applies to, must start with `/`
- `limit` — max requests allowed per window (positive integer)
- `window` — window length in seconds (positive integer)
- `burst` — max requests allowed in a short spike above `limit` (must be >= `limit`)

## Usage

```
$ ratelint api-gateway.rules
api-gateway.rules:8: error: rule 'search' has no explicit 'burst'; add one or pass --lenient to default it to 'limit'
```

The `search` rule above is missing `burst`. In strict mode (the default)
that's an error: ratelint would rather you write down what the spike
allowance actually is than silently inherit whatever the limiter's default
happens to be.

If you're fine with that default and don't want to spell out `burst` on
every rule, opt out explicitly:

```
$ ratelint --lenient api-gateway.rules
api-gateway.rules: no findings (2 rules)
```

`--lenient` only relaxes heuristic checks (missing `burst`, suspiciously
large `limit`/`window` values). It never turns off structural checks like
missing `path`/`limit`/`window`, bad integers, or two rules claiming the
same path — those stay errors either way.

If you'd rather have ratelint fill in the missing `burst` than write it
yourself, pass `--fix`. It rewrites the file in place, defaulting `burst` to
`limit` for any rule that has a valid `limit` but no `burst`, then lints the
result as usual:

```
$ ratelint --fix api-gateway.rules
api-gateway.rules:8: fix: added 'burst = 100' to rule 'search'
api-gateway.rules: no findings (2 rules)
```

`--fix` only touches that one case. A rule with no `limit`, or a `limit`
that isn't a valid positive integer, is left alone — those need a human to
decide the right value, so they still show up as ordinary findings.

For CI, pass `--json` to get a single JSON object on stdout instead of the
line-oriented text output:

```
$ ratelint --json api-gateway.rules
{"file":"api-gateway.rules","rules":2,"fixed":[],"findings":[{"line":8,"severity":"error","message":"rule 'search' has no explicit 'burst'; add one or pass --lenient to default it to 'limit'"}]}
```

Combined with `--fix`, the `fixed` array lists what was changed:

```
$ ratelint --json --fix api-gateway.rules
{"file":"api-gateway.rules","rules":2,"fixed":[{"line":8,"rule":"search","field":"burst","value":100}],"findings":[]}
```

A file that fails to parse produces `{"file":...,"parse_error":{"line":...,"message":...}}`
instead. Exit codes are unaffected by `--json`.

Exit codes: `0` if there are no errors, `1` if any rule produced an error,
`2` for usage problems (bad arguments, unreadable file, malformed rule
file).

## Auto-fixing

`--fix` currently handles one case: a rule with a valid `limit` but no
`burst` gets `burst = <limit>` appended to its section. That's the only
issue in the list below that has an unambiguous correct value to fill in;
everything else needs a person to decide what the right value actually is.

## What it catches today

- missing `path`, `limit`, or `window`
- `limit`/`window`/`burst` that aren't positive integers
- `path` that doesn't start with `/`
- two rules pointing at the same `path`
- `burst` lower than `limit`
- (strict only) no explicit `burst`
- (strict only) `limit` or `window` past a sane ceiling, likely a typo

## Building

Standard library only, no dependencies:

```
cargo build --release
./target/release/ratelint --lenient api-gateway.rules
```

Run the test suite with `cargo test`.

## Status

Early. The rule format and check list are both going to grow — see the
issues for what's planned next.
