& docker run --rm --entrypoint bugboard-mcp bugboard-mcp:local --stdio

# stdio exits with this error after receiving EOF without an initialize request.
if ($LASTEXITCODE -ne 1) {
    exit $LASTEXITCODE
}
