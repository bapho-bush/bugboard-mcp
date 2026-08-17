$randomCookie = "bugboard_session=$([guid]::NewGuid().ToString('N'))"
$httpContainer = "bugboard-mcp-http-smoke-$([guid]::NewGuid().ToString('N'))"

function ConvertFrom-McpHttpResponse([string]$content) {
    $data = $content -split "`r?`n" |
        Where-Object { $_.StartsWith("data:") -and $_.Substring(5).Trim() } |
        Select-Object -First 1
    if ($data) {
        return ($data.Substring(5).Trim() | ConvertFrom-Json)
    }

    return ($content | ConvertFrom-Json)
}

try {
    docker run --detach --rm --name $httpContainer --publish "127.0.0.1::8000" -e "BUGBOARD_COOKIE=$randomCookie" bugboard-mcp:local | Out-Null
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }

    $port = ((docker port $httpContainer 8000/tcp) | Select-Object -First 1) -replace '^127\.0\.0\.1:', ''
    if (-not $port) {
        throw "Docker HTTP smoke could not determine the published port."
    }

    $initialize = @{
        jsonrpc = "2.0"
        id = 1
        method = "initialize"
        params = @{
            protocolVersion = "2025-11-25"
            capabilities = @{}
            clientInfo = @{ name = "docker-http-smoke"; version = "0.0.0" }
        }
    } | ConvertTo-Json -Compress -Depth 4

    $httpResponse = $null
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        try {
            $httpResponse = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$port/mcp" -Method Post -ContentType "application/json" -Headers @{ Accept = "application/json, text/event-stream" } -Body $initialize -TimeoutSec 2
            break
        }
        catch {
            Start-Sleep -Milliseconds 250
        }
    }

    if (-not $httpResponse) {
        throw "Docker HTTP smoke did not receive an initialize response."
    }

    $initializeResponse = ConvertFrom-McpHttpResponse $httpResponse.Content
    if (-not $initializeResponse.result.protocolVersion) {
        throw "Docker HTTP smoke received an invalid initialize response."
    }

    $sessionId = $httpResponse.Headers["Mcp-Session-Id"]
    if (-not $sessionId) {
        throw "Docker HTTP smoke did not receive a session id."
    }

    $tools = '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
    $toolsResponse = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$port/mcp" -Method Post -ContentType "application/json" -Headers @{ Accept = "application/json, text/event-stream"; "Mcp-Session-Id" = $sessionId } -Body $tools -TimeoutSec 10
    $toolsResult = ConvertFrom-McpHttpResponse $toolsResponse.Content
    if (-not ($toolsResult.result.tools.name -contains "bugboard_auth_status")) {
        throw "Docker HTTP smoke received an invalid tools/list response."
    }

    Write-Output "Docker HTTP smoke: initialize and tools/list succeeded."
}
finally {
    docker stop $httpContainer 2>$null | Out-Null
}

$messages = @'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"docker-smoke","version":"0.0.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"project_list","arguments":{}}}
'@

$output = $messages | docker run --rm -i -e "BUGBOARD_MCP_TRANSPORT=stdio" -e "BUGBOARD_COOKIE=$randomCookie" bugboard-mcp:local
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

Write-Output "Docker stdio smoke: received explicit not_authenticated."
