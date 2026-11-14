# Runs the installed setup program, or downloads @@VERSION@@ and verifies its SHA-256 and
# Authenticode signature on a fresh install. Runs in Windows PowerShell 5.1 and PowerShell 7.
#
#   irm https://static.openvulcan.com/vulcancode/install.ps1 | iex
#   & ([scriptblock]::Create((irm https://static.openvulcan.com/vulcancode/install.ps1))) --no-gui --yes
#   powershell -ExecutionPolicy Bypass -File install.ps1 --no-gui --yes
#
# --help lists the setup program's arguments. This file is a template:
# scripts/entry-scripts.mjs fills in the version, the origins, the digest and the signer when a setup version
# is published. It stays ASCII so Windows PowerShell 5.1 reads it the same in every code page, and it
# works in a script block so nothing it sets leaks into the session that ran `iex`, except the user PATH
# entries the setup program added: they are copied into this session so the commands work at once.

& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    $Version = '@@VERSION@@'
    $Origins = @(@@ORIGINS@@)
    $Digest = '@@SHA256_WINDOWS_X64@@'
    $Signer = '@@SIGNER@@'

    # A 32-bit PowerShell on 64-bit Windows reports its own architecture in PROCESSOR_ARCHITECTURE.
    $Architecture = $env:PROCESSOR_ARCHITEW6432
    if (-not $Architecture) { $Architecture = $env:PROCESSOR_ARCHITECTURE }
    if ($Architecture -ne 'AMD64') {
        throw "Vulcan Code for Windows is built for x64; this computer reports $Architecture"
    }
    # Windows PowerShell 5.1 may not offer TLS 1.2 by default; it is added, nothing is removed.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    # Show-Received writes how much of the setup program has arrived over the line it wrote last; a console
    # that is redirected gets only the final size.
    function Show-Received([long] $Received, [long] $Total, [bool] $Final) {
        $Text = '  {0:N1} MB' -f ($Received / 1MB)
        if ($Total -gt 0) { $Text = '  {0:N1} / {1:N1} MB ({2}%)' -f ($Received / 1MB), ($Total / 1MB), [int](100 * $Received / $Total) }
        if (-not [Console]::IsOutputRedirected) { Write-Host -NoNewline "`r$Text" }
        if ($Final) { if ([Console]::IsOutputRedirected) { Write-Host $Text } else { Write-Host '' } }
    }

    # Receive-Program downloads $Uri to $Path with that progress. An origin that does not answer, or stops
    # sending, for 30 seconds fails, so the next one is tried instead of waiting without end.
    function Receive-Program([string] $Uri, [string] $Path) {
        $Request = [Net.WebRequest]::Create($Uri)
        $Request.Timeout = 30000
        $Request.ReadWriteTimeout = 30000
        $Response = $Request.GetResponse()
        try {
            $Stream = $Response.GetResponseStream()
            $Output = [IO.File]::Create($Path)
            try {
                $Buffer = New-Object byte[] 65536
                $Received = 0L
                Show-Received 0 $Response.ContentLength $false
                $Clock = [Diagnostics.Stopwatch]::StartNew()
                while (($Count = $Stream.Read($Buffer, 0, $Buffer.Length)) -gt 0) {
                    $Output.Write($Buffer, 0, $Count)
                    $Received += $Count
                    if ($Clock.ElapsedMilliseconds -ge 200) {
                        Show-Received $Received $Response.ContentLength $false
                        $Clock.Restart()
                    }
                }
                Show-Received $Received $Response.ContentLength $true
            } finally {
                $Output.Dispose()
                $Stream.Dispose()
            }
        } finally {
            $Response.Dispose()
        }
    }

    # Match the install root and value-taking arguments in paths::default_root and args::parse.
    $LocalData = [Environment]::GetFolderPath('LocalApplicationData')
    $Root = if ($LocalData) { Join-Path $LocalData 'Programs\VulcanCode' }
    $AutoUpdate = $args.Count -eq 0 -or $args[0] -cin @('install', 'update') -or $args[0] -clike '-*'
    $ValueFor = $null
    # An omitted route uses the published order for the bootstrap download.
    # The installer also receives the original arguments and uses its recorded route when omitted.
    $Route = 'auto'
    foreach ($Arg in $args) {
        if ($ValueFor) {
            if ($ValueFor -ceq '--prefix') { $Root = $Arg }
            if ($ValueFor -ceq '--route') { $Route = $Arg }
            $ValueFor = $null
            continue
        }
        if ($Arg -clike '--prefix=*') { $Root = $Arg.Substring('--prefix='.Length) }
        if ($Arg -clike '--route=*') { $Route = $Arg.Substring('--route='.Length) }
        if ($Arg -cin @('--prefix', '--mode', '--port-range', '--with', '--channel', '--route')) { $ValueFor = $Arg }
        if ($Arg -cin @('--self-update', '--no-self-update')) { $AutoUpdate = $false }
    }
    if ($ValueFor -ceq '--route') { throw '--route needs a value' }
    switch -CaseSensitive ($Route) {
        'auto' {}
        'international' { $Origins = @($Origins[0]) }
        'china' { $Origins = @($Origins[1]) }
        default { throw '--route accepts auto, international or china' }
    }
    $Program = $null
    if ($Root -and -not $ValueFor) {
        $RootPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Root)
        $SetupDir = Join-Path $RootPath 'setup'
        $RootItem = Get-Item -LiteralPath $RootPath -ErrorAction SilentlyContinue
        $SetupDirItem = Get-Item -LiteralPath $SetupDir -ErrorAction SilentlyContinue
        $State = Get-Item -LiteralPath (Join-Path $SetupDir 'installation.json') -ErrorAction SilentlyContinue
        $Installed = Get-Item -LiteralPath (Join-Path $SetupDir 'vulcan-code-setup.exe') -ErrorAction SilentlyContinue
        $ReparsePoint = [IO.FileAttributes]::ReparsePoint
        if ($RootItem.PSIsContainer -and -not ($RootItem.Attributes -band $ReparsePoint) -and
            $SetupDirItem.PSIsContainer -and -not ($SetupDirItem.Attributes -band $ReparsePoint) -and
            $State -and -not $State.PSIsContainer -and -not ($State.Attributes -band $ReparsePoint) -and
            $Installed -and -not $Installed.PSIsContainer -and -not ($Installed.Attributes -band $ReparsePoint)) {
            $Program = $Installed.FullName
        }
    }

    # Fresh installs and repairs still use the downloaded and verified program.
    $SetupArgs = @($args)
    $Work = $null
    try {
        if ($Program) {
            Write-Host 'Starting the installed Vulcan Code setup program'
            if ($AutoUpdate) { $SetupArgs += '--self-update' }
        } else {
            $File = "vulcan-code-setup-$Version-windows-x64.exe"
            $Work = Join-Path ([IO.Path]::GetTempPath()) ('vulcan-code-setup-' + [Guid]::NewGuid().ToString('N'))
            New-Item -ItemType Directory -Path $Work | Out-Null
            $Program = Join-Path $Work 'vulcan-code-setup.exe'
            # The origins are tried in order; the mirror serves the same verified objects.
            $Fetched = $false
            foreach ($Origin in $Origins) {
                Write-Host "Downloading the Vulcan Code setup program $Version from $Origin"
                try {
                    Receive-Program "$Origin/setup/$Version/$File" $Program
                    $Fetched = $true
                    break
                } catch {
                    if (-not [Console]::IsOutputRedirected) { Write-Host '' }
                    $Reason = $_.Exception
                    while ($Reason.InnerException) { $Reason = $Reason.InnerException }
                    if ($Route -ceq 'auto') {
                        Write-Warning "Cannot download from $Origin ($($Reason.Message)), trying the next origin"
                    } else {
                        Write-Warning "Cannot download from $Origin ($($Reason.Message))"
                    }
                }
            }
            if (-not $Fetched) { throw "Cannot download $File" }
            $Actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Program).Hash.ToLowerInvariant()
            if ($Actual -ne $Digest) {
                throw "$File does not match its published SHA-256 (expected $Digest, got $Actual)"
            }
            if ($Signer) {
                $Signature = Get-AuthenticodeSignature -LiteralPath $Program
                if ($Signature.Status -ne 'Valid' -or $Signature.SignerCertificate.Thumbprint -ne $Signer) {
                    throw "$File is not signed by the Vulcan Code certificate ($($Signature.Status))"
                }
            }
            Write-Host "Starting the setup program $Version"
        }
        & $Program @SetupArgs
        # 130 is CANCELLED_EXIT_CODE of apps/vulcan-code-setup: the user left it and nothing was changed.
        if ($LASTEXITCODE -eq 130) { return }
        if ($LASTEXITCODE -ne 0) { throw "vulcan-code-setup exited with $LASTEXITCODE" }
        # A program cannot change the environment of the shell that started it, so the user PATH
        # entries missing here are added to this session; under `iex` that is the user's own window.
        $Session = @($env:Path -split ';' | Where-Object { $_ } | ForEach-Object { $_.TrimEnd('\') })
        $Added = $false
        foreach ($Entry in ([Environment]::GetEnvironmentVariable('Path', 'User') -split ';')) {
            $Entry = [Environment]::ExpandEnvironmentVariables($Entry).TrimEnd('\')
            if ($Entry -and $Session -notcontains $Entry) {
                $env:Path = ($env:Path.TrimEnd(';') + ';' + $Entry)
                $Session += $Entry
                $Added = $true
            }
        }
        if ($Added) { Write-Host 'vulcan-tui and vulcan-code-setup work in this PowerShell session now.' }
    } finally {
        if ($Work) { Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue }
    }
} @args
