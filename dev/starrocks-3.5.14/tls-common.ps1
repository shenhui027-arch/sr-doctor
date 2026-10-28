Set-StrictMode -Version Latest

# TLS variant of the allin1 fixture (v0.7-01). Dot-source after common.ps1.
#
# Official 3.5.14 MySQL-protocol SSL (source 23a56ec, fe/fe-core/.../common/Config.java
# and mysql/ssl/SSLContextLoader.java; docs/en/administration/user_privs/ssl_authentication.md):
# the FE loads `ssl_keystore_location` with KeyStore.getInstance("JKS"), unlocks it
# with `ssl_keystore_password`, reads the key with `ssl_key_password`, and
# `ssl_force_secure_transport = true` rejects unencrypted clients. JDK 17's JKS
# keystore also reads PKCS#12 (keystore.type.compat), which is the format the
# documented `keytool -genkeypair` writes by default.

$script:TlsFixtureContainerName = 'sr-doctor-v0.1-e2e-3514-tls'
$script:TlsFixtureContainerDir = '/data/deploy/tls'
$script:TlsFixtureFeConf = '/data/deploy/starrocks/fe/conf/fe.conf'
$script:TlsFixtureServerName = 'localhost'

function New-TlsFixtureCertificateAuthority {
    param([Parameter(Mandatory)][string]$CommonName, [Parameter(Mandatory)][DateTimeOffset]$NotBefore, [Parameter(Mandatory)][DateTimeOffset]$NotAfter)
    $key = [Security.Cryptography.RSA]::Create(2048)
    $request = [Security.Cryptography.X509Certificates.CertificateRequest]::new("CN=$CommonName", $key, [Security.Cryptography.HashAlgorithmName]::SHA256, [Security.Cryptography.RSASignaturePadding]::Pkcs1)
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new($true, $true, 0, $true))
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new(
        [Security.Cryptography.X509Certificates.X509KeyUsageFlags]::KeyCertSign -bor [Security.Cryptography.X509Certificates.X509KeyUsageFlags]::CrlSign, $true))
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509SubjectKeyIdentifierExtension]::new($request.PublicKey, $false))
    $request.CreateSelfSigned($NotBefore, $NotAfter)
}

function New-TlsFixtureServerKeystore {
    param(
        [Parameter(Mandatory)][Security.Cryptography.X509Certificates.X509Certificate2]$Issuer,
        [Parameter(Mandatory)][DateTimeOffset]$NotBefore,
        [Parameter(Mandatory)][DateTimeOffset]$NotAfter,
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Password
    )
    $key = [Security.Cryptography.RSA]::Create(2048)
    $request = [Security.Cryptography.X509Certificates.CertificateRequest]::new("CN=$script:TlsFixtureServerName", $key, [Security.Cryptography.HashAlgorithmName]::SHA256, [Security.Cryptography.RSASignaturePadding]::Pkcs1)
    $san = [Security.Cryptography.X509Certificates.SubjectAlternativeNameBuilder]::new()
    $san.AddDnsName($script:TlsFixtureServerName)
    $request.CertificateExtensions.Add($san.Build($false))
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new($false, $false, 0, $true))
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new(
        [Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature -bor [Security.Cryptography.X509Certificates.X509KeyUsageFlags]::KeyEncipherment, $true))
    $serverAuth = [Security.Cryptography.OidCollection]::new()
    [void]$serverAuth.Add([Security.Cryptography.Oid]::new('1.3.6.1.5.5.7.3.1'))
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($serverAuth, $false))
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509AuthorityKeyIdentifierExtension]::CreateFromCertificate($Issuer, $true, $false))
    $serial = [byte[]]::new(16); [Security.Cryptography.RandomNumberGenerator]::Fill($serial); $serial[0] = $serial[0] -band 0x7f
    $leaf = $request.Create($Issuer, $NotBefore, $NotAfter, $serial)
    $withKey = [Security.Cryptography.X509Certificates.RSACertificateExtensions]::CopyWithPrivateKey($leaf, $key)
    $chain = [Security.Cryptography.X509Certificates.X509Certificate2Collection]::new()
    [void]$chain.Add($withKey)
    [void]$chain.Add([Security.Cryptography.X509Certificates.X509Certificate2]::new($Issuer.RawData))
    [IO.File]::WriteAllBytes($Path, $chain.Export([Security.Cryptography.X509Certificates.X509ContentType]::Pkcs12, $Password))
    $key.Dispose()
    [ordered]@{ not_before_utc = $NotBefore.UtcDateTime.ToString('o'); not_after_utc = $NotAfter.UtcDateTime.ToString('o'); subject_alt_name = "DNS:$script:TlsFixtureServerName"; sha256 = $leaf.GetCertHashString([Security.Cryptography.HashAlgorithmName]::SHA256).ToLowerInvariant() }
}

function Write-TlsFixturePem {
    param([Parameter(Mandatory)][Security.Cryptography.X509Certificates.X509Certificate2]$Certificate, [Parameter(Mandatory)][string]$Path)
    [IO.File]::WriteAllText($Path, $Certificate.ExportCertificatePem() + "`n", [Text.UTF8Encoding]::new($false))
}

# One run's TLS material. Only public CA certificates stay on the host for the
# product's trust anchor; both server keystores carry private keys and are
# copied into the (not yet started) container, after which the caller deletes
# the host directory.
function New-TlsFixtureMaterial {
    param([Parameter(Mandatory)][string]$Directory, [Parameter(Mandatory)][string]$KeystorePassword)
    New-Item -ItemType Directory -Force -Path $Directory | Out-Null
    $now = [DateTimeOffset]::UtcNow
    $ca = New-TlsFixtureCertificateAuthority 'sr-doctor v0.7 TLS E2E CA' $now.AddDays(-7) $now.AddDays(2)
    $untrusted = New-TlsFixtureCertificateAuthority 'sr-doctor v0.7 TLS E2E unrelated CA' $now.AddDays(-7) $now.AddDays(2)
    $keys = Join-Path $Directory 'keystores'
    New-Item -ItemType Directory -Force -Path $keys | Out-Null
    $valid = New-TlsFixtureServerKeystore $ca $now.AddHours(-1) $now.AddDays(1) (Join-Path $keys 'valid.p12') $KeystorePassword
    $expired = New-TlsFixtureServerKeystore $ca $now.AddDays(-3) $now.AddDays(-1) (Join-Path $keys 'expired.p12') $KeystorePassword
    Copy-Item -LiteralPath (Join-Path $keys 'valid.p12') -Destination (Join-Path $keys 'server.p12')
    $caPem = Join-Path $Directory 'fixture-ca.pem'
    $untrustedPem = Join-Path $Directory 'unrelated-ca.pem'
    Write-TlsFixturePem $ca $caPem
    Write-TlsFixturePem $untrusted $untrustedPem
    $result = [ordered]@{
        keystore_directory = $keys; ca_pem = $caPem; unrelated_ca_pem = $untrustedPem
        ca_sha256 = $ca.GetCertHashString([Security.Cryptography.HashAlgorithmName]::SHA256).ToLowerInvariant()
        unrelated_ca_sha256 = $untrusted.GetCertHashString([Security.Cryptography.HashAlgorithmName]::SHA256).ToLowerInvariant()
        valid_leaf = $valid; expired_leaf = $expired; keystore_format = 'pkcs12'
    }
    $ca.Dispose(); $untrusted.Dispose()
    $result
}

# Create, configure and start the TLS fixture. The FE reads fe.conf only at
# start, so the keystores and SSL settings are copied into the created
# container before `docker start`. Only the MySQL port is published.
function Start-TlsFixture {
    param(
        [Parameter(Mandatory)][string]$DockerExecutable,
        [Parameter(Mandatory)][string]$KeystoreDirectory,
        [Parameter(Mandatory)][string]$KeystorePassword,
        [Parameter(Mandatory)][string]$DataRoot,
        [Parameter(Mandatory)][int]$MySqlPort,
        [ValidateRange(30, 300)][int]$ReadyTimeoutSeconds = 180,
        [switch]$LowResource
    )
    $name = $script:TlsFixtureContainerName
    if ($null -ne (Get-FixtureContainerIdentity -DockerExecutable $DockerExecutable -ContainerName $name)) { throw 'The TLS fixture container already exists.' }
    $data = Assert-FixtureDataRoot $DataRoot
    $feMeta = Join-Path $data 'fe-meta'
    $beStorage = Join-Path $data 'be-storage'
    foreach ($directory in @($feMeta, $beStorage)) { New-Item -ItemType Directory -Force -Path $directory | Out-Null }
    Invoke-FixtureDocker $DockerExecutable @('image', 'inspect', $script:FixtureImage) -DiscardOutput
    Invoke-FixtureDocker $DockerExecutable (@(
        'create', '--name', $name,
        '--label', "$script:FixtureLabelKey=$script:FixtureLabelValue"
    ) + (Get-FixtureResourceArguments -LowResource:$LowResource) + @(
        '--pids-limit', '1024', '--security-opt', 'no-new-privileges',
        '--publish', "127.0.0.1:$MySqlPort`:9030",
        '--volume', "${feMeta}:/data/deploy/starrocks/fe/meta",
        '--volume', "${beStorage}:/data/deploy/starrocks/be/storage",
        $script:FixtureImage
    )) -DiscardOutput
    if ($LowResource) { Set-FixtureLowResourceConfig -DockerExecutable $DockerExecutable -ContainerName $name }
    $staging = Join-Path ([IO.Path]::GetTempPath()) ('starrocks-tls-feconf-' + [Guid]::NewGuid().ToString('N'))
    try {
        New-Item -ItemType Directory -Force -Path $staging | Out-Null
        $feConf = Join-Path $staging 'fe.conf'
        Invoke-FixtureDocker $DockerExecutable @('cp', "${name}:$script:TlsFixtureFeConf", $feConf) -DiscardOutput
        $ssl = @(
            '', '# v0.7-01 TLS E2E fixture: official 3.5.14 MySQL-protocol SSL settings.',
            "ssl_keystore_location = $script:TlsFixtureContainerDir/server.p12",
            "ssl_keystore_password = $KeystorePassword",
            "ssl_key_password = $KeystorePassword",
            'ssl_force_secure_transport = true', ''
        ) -join "`n"
        [IO.File]::AppendAllText($feConf, $ssl, [Text.UTF8Encoding]::new($false))
        Invoke-FixtureDocker $DockerExecutable @('cp', $feConf, "${name}:$script:TlsFixtureFeConf") -DiscardOutput
    }
    finally {
        if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
    }
    Invoke-FixtureDocker $DockerExecutable @('cp', $KeystoreDirectory, "${name}:$script:TlsFixtureContainerDir") -DiscardOutput
    Invoke-FixtureDocker $DockerExecutable @('start', $name) -DiscardOutput
    $version = Wait-TlsFixtureReady -DockerExecutable $DockerExecutable -ReadyTimeoutSeconds $ReadyTimeoutSeconds -RequireBackend
    [ordered]@{ container = $name; image = $script:FixtureImage; image_digest = $script:FixtureDigest; observed_version = $version; host_ports = [ordered]@{ mysql = $MySqlPort }; data_root = $data; resources = Get-FixtureResourceReceipt -LowResource:$LowResource }
}

# The in-container mysql 8.0 client negotiates TLS (ssl-mode PREFERRED, no
# certificate check), which ssl_force_secure_transport requires.
function Wait-TlsFixtureReady {
    param([Parameter(Mandatory)][string]$DockerExecutable, [int]$ReadyTimeoutSeconds = 180, [switch]$RequireBackend)
    $name = $script:TlsFixtureContainerName
    $deadline = [DateTime]::UtcNow.AddSeconds($ReadyTimeoutSeconds)
    $version = $null
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $version = (Invoke-FixtureMysql $DockerExecutable $name 'root' '' 'SELECT current_version();' | Select-Object -First 1).Trim()
            if ($version -match '^3\.5\.14(?:-|$)') { break }
        } catch { }
        Start-Sleep -Seconds 2
    }
    if ($version -notmatch '^3\.5\.14(?:-|$)') { throw 'The TLS fixture FE did not become a StarRocks 3.5.14 server before the readiness deadline.' }
    if ($RequireBackend) {
        $alive = $false
        while ([DateTime]::UtcNow -lt $deadline) {
            try {
                $backends = @(Invoke-FixtureMysql $DockerExecutable $name 'root' '' 'SHOW BACKENDS;')
                if (@($backends | Where-Object { ([string]$_ -split "`t")[8] -eq 'true' }).Count -gt 0) { $alive = $true; break }
            } catch { }
            Start-Sleep -Seconds 2
        }
        if (-not $alive) { throw 'The TLS fixture BE did not report alive before the readiness deadline.' }
    }
    $version
}

function Assert-TlsFixtureSecureTransport {
    param([Parameter(Mandatory)][string]$DockerExecutable)
    # The fixture's own check, separate from the product's handshake: a client
    # that requires SSL reports the negotiated cipher (StarRocks has no
    # `Ssl_cipher` status variable). In-container clients connect from
    # 127.0.0.1, which 3.5.14 MysqlProto.negotiate() exempts from
    # ssl_force_secure_transport, so only REQUIRED proves the FE serves SSL.
    $status = @(Invoke-FixtureDocker $DockerExecutable @('exec', $script:TlsFixtureContainerName, 'mysql', '--host=127.0.0.1', '--port=9030', '--user=root', '--ssl-mode=REQUIRED', '--execute=status'))
    $cipher = @($status | ForEach-Object { if ([string]$_ -match 'Cipher in use is (\S+)') { $Matches[1] } }) | Select-Object -First 1
    [ordered]@{ in_container_client_cipher = $cipher }
}

# Count of FE refusals of an unencrypted client (MysqlProto.negotiate():133).
function Get-TlsFixtureInsecureRefusalCount {
    param([Parameter(Mandatory)][string]$DockerExecutable)
    # grep exits 1 on zero matches; the count is still printed.
    $count = @(Invoke-FixtureDocker $DockerExecutable @('exec', $script:TlsFixtureContainerName, 'sh', '-c', "grep -c 'Connections using insecure transport are prohibited' /data/deploy/starrocks/fe/log/fe.log || true") | Select-Object -First 1)
    [int]([string]$count[0]).Trim()
}

function New-TlsFixtureReader {
    param([Parameter(Mandatory)][string]$DockerExecutable, [Parameter(Mandatory)][string]$Secret)
    $hash = Get-FixtureNativePasswordHash (Get-FixturePassword $Secret 'doctor_reader')
    try {
        Invoke-FixtureMysql $DockerExecutable $script:TlsFixtureContainerName 'root' '' "CREATE USER IF NOT EXISTS doctor_reader IDENTIFIED BY PASSWORD $(ConvertTo-SqlLiteral $hash);" | Out-Null
    }
    catch { throw 'TLS fixture reader account creation failed.' }
}

# Swap the served certificate for the expired one and restart only the FE.
function Use-TlsFixtureExpiredCertificate {
    param([Parameter(Mandatory)][string]$DockerExecutable, [int]$ReadyTimeoutSeconds = 180)
    $name = $script:TlsFixtureContainerName
    Invoke-FixtureDocker $DockerExecutable @('exec', $name, 'cp', "$script:TlsFixtureContainerDir/expired.p12", "$script:TlsFixtureContainerDir/server.p12") -DiscardOutput
    Invoke-FixtureDocker $DockerExecutable @('exec', $name, 'supervisorctl', 'restart', 'feservice') -DiscardOutput
    Wait-TlsFixtureReady -DockerExecutable $DockerExecutable -ReadyTimeoutSeconds $ReadyTimeoutSeconds | Out-Null
}
