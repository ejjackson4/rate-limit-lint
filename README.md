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
{"file":"api-gateway.rules","rules":2,"fixed":[],"findings":[{"line":8,"severity":"error","code":"missing-burst","message":"rule 'search' has no explicit 'burst'; add one or pass --lenient to default it to 'limit'"}]}
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

## Alternate input formats

Most of the time the rate limits already live somewhere else - an nginx
config, say - and the INI file above would just be a second copy that drifts
out of sync. Pass `--format=nginx` to lint the config directly instead of
maintaining a separate rules file:

```
$ ratelint --format=nginx nginx.conf
```

ratelint reads `limit_req_zone` directives for the rate (`rate=5r/s` or
`rate=300r/m`, converted to `limit`/`window`) and `limit_req` directives
inside `location` blocks for the path and `burst`. Everything else in the
file - `server`, `http`, `listen`, unrelated directives - is skipped. The
same checks run afterward: missing burst, absurd limits, duplicate paths,
and so on, same as the native format.

Two things this format needs that the INI one doesn't:

- one directive or block opener/closer per line - `location /x { limit_req
  zone=y; }` packed onto a single line isn't parsed
- a `limit_req` naming a zone with no matching `limit_req_zone` is a parse
  error, not a lint finding, since there's no rate to check anything against

`--fix` isn't available for this format yet - the missing-burst diagnostic
still fires, but there's nowhere unambiguous to rewrite the source, since a
`limit_req` line's `burst=` argument sits next to directives ratelint
doesn't otherwise touch.

`--format=envoy` reads the descriptor config used by the
[envoyproxy/ratelimit](https://github.com/envoyproxy/ratelimit) sidecar:

```yaml
domain: api-gateway
descriptors:
  - key: path
    value: /api/login
    rate_limit:
      unit: second
      requests_per_unit: 5
```

Only descriptors with `key: path` become rules - `value` is read as the
path, and `rate_limit.requests_per_unit`/`unit` become `limit`/`window`
(`unit` must be `second`, `minute`, `hour`, or `day`). Other descriptor
kinds, like `remote_address` or `header_match`, are read past and ignored,
since there's nothing to check them against. This format has no `burst`
concept, so every rule parses with `burst` unset, which means the
missing-burst check always fires unless you pass `--lenient`. As with
`--format=nginx`, `--fix` isn't available here.

Nested `descriptors:` (per-key sub-limits some envoy configs use for
compound rate limits) aren't supported - every descriptor in the list is
expected to carry its own `rate_limit` directly.

## Auto-fixing

`--fix` currently handles one case: a rule with a valid `limit` but no
`burst` gets `burst = <limit>` appended to its section. That's the only
issue in the list below that has an unambiguous correct value to fill in;
everything else needs a person to decide what the right value actually is.

## What it catches today

- missing `path`, `limit`, or `window` (`missing-path`, `missing-limit`, `missing-window`)
- `limit`/`window`/`burst` that aren't positive integers (`invalid-value`)
- `path` that doesn't start with `/` (`path-format`)
- two rules pointing at the same `path` (`duplicate-path`)
- `burst` lower than `limit` (`burst-below-limit`)
- (strict only) no explicit `burst` (`missing-burst`)
- (strict only) `limit` or `window` past a sane ceiling, likely a typo (`limit-too-large`, `window-too-large`)

The name in parentheses is the check's stable id, used by `--suppress` below
and printed as `code` in `--json` output.

## Suppressing specific checks

Sometimes one rule needs an exception rather than a blanket relaxation - a
legacy endpoint whose burst really is meant to fall back to the limiter's
default, a batch-import path that legitimately needs a six-figure limit.
`--lenient` would turn that check off for every rule in the file. Pass
`--suppress` with a file naming exactly which check to skip on which rule
instead:

```
# suppressions.txt
legacy-export: missing-burst
bulk-import: limit-too-large
```

```
$ ratelint --suppress suppressions.txt api-gateway.rules
```

Each line is `rule: check[, check...]`. `#` starts a comment, blank lines
are ignored, and `*` as the rule name suppresses a check across every rule
in the file:

```
# every rule in this file intentionally exceeds the sane window ceiling
*: window-too-large
```

A suppressed finding is dropped entirely - it isn't printed and doesn't
count toward the exit code. A check name that isn't one of the ids listed
above is a usage error rather than a silent no-op, since a typo in a
suppression file would otherwise mean the check quietly stops running and
nobody notices.

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
