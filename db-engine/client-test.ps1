$ErrorActionPreference = "Stop"

$client = New-Object System.Net.Sockets.TcpClient
$client.Connect("127.0.0.1", 9000)

$stream = $client.GetStream()
$enc    = New-Object System.Text.UTF8Encoding($false)
$writer = New-Object System.IO.StreamWriter($stream, $enc)
$writer.AutoFlush = $true
$reader = New-Object System.IO.StreamReader($stream, $enc)

function Send-Sql([string]$sql) {
    Write-Host "SQL> $sql" -ForegroundColor Cyan
    $writer.WriteLine($sql)

    $header = $reader.ReadLine()
    if ($null -eq $header) {
        Write-Host "  <- (no response)" -ForegroundColor Red
        return
    }

    if ($header.StartsWith("ROWS")) {
        $count = [int]($header.Split(" ")[2])
        $cols  = $reader.ReadLine()
        Write-Host ("     " + $cols.Replace("`t", " | "))
        for ($i = 0; $i -lt $count; $i++) {
            $row = $reader.ReadLine()
            Write-Host ("     " + $row.Replace("`t", " | "))
        }
        $null = $reader.ReadLine()
        Write-Host "     END"
    }
    else {
        Write-Host "  <- $header" -ForegroundColor Yellow
    }
}

$t = "t" + (Get-Random -Maximum 999)

Send-Sql "CREATE TABLE $t (id INT PRIMARY KEY, name VARCHAR(50))"
Send-Sql "INSERT INTO $t VALUES (1, 'Alice')"
Send-Sql "INSERT INTO $t VALUES (2, 'Bob')"
Send-Sql "SELECT * FROM $t"

Send-Sql "BEGIN"
Send-Sql "INSERT INTO $t VALUES (3, 'Carol')"
Send-Sql "SELECT * FROM $t"
Send-Sql "ROLLBACK"
Send-Sql "SELECT * FROM $t"

Send-Sql "quit"
$client.Close()
Write-Host ""
Write-Host "DONE - client closed." -ForegroundColor Green