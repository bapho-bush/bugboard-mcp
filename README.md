# bugboard-mcp

`bugboard-mcp` gives an MCP client access to projects, versions, bugs, history,
subscriptions, and votes in the 1C Bugboard. It uses the browser session you
already have; it does not collect credentials or automate the browser.

The server is unofficial and experimental. Use it with an account that is
allowed to access the target Bugboard data.

## What it does

- Lists projects, versions, recent and subscribed bugs, and bugs you voted for.
- Searches by bug number or full-text query, then returns bug details and
  reference-redacted history.
- Creates session-local opaque handles so MCP clients never receive Bugboard
  references.
- Subscribes, unsubscribes, votes, and unvotes safely: every write checks the
  current state, skips a no-op, and confirms the result.

The default transport is Streamable HTTP at `http://127.0.0.1:8000/mcp`. For
local integrations and smoke checks, use stdio instead.

## Run it

Install the pinned toolchain:

```sh
mise install
```

Set `BUGBOARD_COOKIE` directly, or create an env-file outside the repository:

```text
BUGBOARD_COOKIE=...
```

Start the server with a direct value:

```powershell
$env:BUGBOARD_COOKIE="..."
mise run run
```

Or point it to an env-file:

```powershell
$env:BUGBOARD_SESSION_ENV="C:\path\outside\repo\bugboard.env"
mise run run
```

The server uses a direct `BUGBOARD_COOKIE` in preference to the env-file. It
loads only `BUGBOARD_COOKIE` from that file. It obtains the
deployment-specific `X-G5-Version` from the authenticated Bugboard shell and
keeps it in memory. Replace an expired cookie and restart the server. A read
may retry once after a Bugboard deployment change; a write never retries on its
own.

Use `mise run run:stdio` to use stdio. `BUGBOARD_MCP_BIND` can change the
address or port.

## Tool inputs

List and search tools return opaque handles. Handles work only in the MCP
session that created them. `bug_get*` and `bug_open_in_browser` take either a
`bug_handle` or a `bug_number`; write tools require `bug_handle`, and vote
tools also require `vote_kind`. `project_get_versions`, `project_subscribe`,
and `project_unsubscribe` require `project_handle`. `version_get_bugs` needs a
project handle and the exact version title returned by `project_get_versions`.

`bug_list_recent` returns the first page. `bug_search` requires a query:
ASCII digits select an exact bug number, while other text runs a full-text
search. Limits range from 1 to 50. Pagination will be added only after its
request shape is verified.

## Development

Run the complete local and CI check:

```sh
mise run verify
```

The workflow in `.github/workflows/ci.yml` runs the same command on Ubuntu and
Windows. The wire-level crate is documented in
[`crates/e1c-element-rpc/README.md`](crates/e1c-element-rpc/README.md).

## Docker

The image runs Streamable HTTP by default on port 8000. Publish it only to the
host loopback interface, because the MCP endpoint has no separate client
authentication. Build it with:

```sh
mise run docker:build
```

Pushes to `main` publish `ghcr.io/bapho-bush/bugboard-mcp:latest` and an
immutable Git commit tag. While the repository is private, pull it with a
GitHub token that has `read:packages`:

```sh
docker pull ghcr.io/bapho-bush/bugboard-mcp:latest
```

Run HTTP MCP for Codex:

```powershell
docker run --rm -d --name bugboard-mcp `
  -p 127.0.0.1:18080:8000 `
  -e "BUGBOARD_COOKIE=$env:BUGBOARD_COOKIE" `
  ghcr.io/bapho-bush/bugboard-mcp:latest
```

Configure Codex with `url = "http://127.0.0.1:18080/mcp"`. Do not use
`-p 18080:8000`, which publishes the unauthenticated MCP endpoint beyond the
local machine.

Run stdio instead by overriding the transport:

```powershell
docker run --rm -i `
  -e "BUGBOARD_MCP_TRANSPORT=stdio" `
  -e "BUGBOARD_COOKIE=$env:BUGBOARD_COOKIE" `
  ghcr.io/bapho-bush/bugboard-mcp:latest
```

An external session file remains supported for either transport when mounting
it is preferable:

```powershell
docker run --rm -d --name bugboard-mcp `
  -p 127.0.0.1:18080:8000 `
  --mount "type=bind,src=C:\path\outside\repo\bugboard.env,dst=/run/secrets/bugboard.env,readonly" `
  -e BUGBOARD_SESSION_ENV=/run/secrets/bugboard.env `
  ghcr.io/bapho-bush/bugboard-mcp:latest
```

## Security

Keep cookies, tokens, browser profiles, and authenticated request bodies out
of the repository. Publish a Docker HTTP port only to host loopback. Browser
requests with an `Origin` header must use an allowed origin.

## License

This project is source-available under [Apache-2.0 with Commons Clause
1.0](LICENSE). You may use, modify, and redistribute it, including for internal
commercial work. You may not sell the software itself or a product or service
whose value substantially comes from its functionality. It is not an
OSI-approved open-source license.
