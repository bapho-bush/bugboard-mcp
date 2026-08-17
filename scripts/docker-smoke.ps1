$randomCookie = "bugboard_session=$([guid]::NewGuid().ToString('N'))"
$messages = @'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"docker-smoke","version":"0.0.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"project_list","arguments":{}}}
'@

$output = $messages | docker run --rm -i -e "BUGBOARD_COOKIE=$randomCookie" bugboard-mcp:local
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

$response = $output |
    ForEach-Object { $_ | ConvertFrom-Json } |
    Where-Object { $_.id -eq 2 } |
    Select-Object -First 1

if ($response.result.isError -ne $true -or $response.result.structuredContent.error.code -ne "not_authenticated") {
    throw "Docker auth smoke expected explicit not_authenticated, got: $($response | ConvertTo-Json -Compress)"
}

Write-Output "Docker auth smoke: received explicit not_authenticated."
