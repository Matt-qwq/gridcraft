# Open a GridCraft-written workbook in an independent spreadsheet application, recalculate it and
# save a copy, so `cargo xtask oracle` can read the copy back and compare it with the model.
#
# WPS through COM stands in for Excel (see docs/oracle.md: Excel is not available on the dev
# machine). Exit codes: 0 = a copy was written, 2 = the application is not installed, 1 = it
# refused or failed.
param([Parameter(Mandatory=$true)][string]$Src, [Parameter(Mandatory=$true)][string]$Dst)

$ErrorActionPreference = 'Stop'

try {
    $app = New-Object -ComObject KET.Application
} catch {
    [Console]::Error.WriteLine("the application is not installed (no KET.Application)")
    exit 2
}

try {
    $app.Visible = $false
    $app.DisplayAlerts = $false
    try { $app.CalculateBeforeSave = $true } catch { }
    if (Test-Path $Dst) { Remove-Item $Dst -Force }
    $wb = $app.Workbooks.Open($Src)
    try {
        try { $wb.Application.CalculateFullRebuild() } catch { $wb.Application.Calculate() }
    } catch {
        [Console]::Error.WriteLine("recalculation failed: $_")
    }
    # 51 = xlOpenXMLWorkbook
    $wb.SaveAs($Dst, 51)
    $wb.Close($false)
} catch {
    [Console]::Error.WriteLine("open/save failed: $_")
    try { $app.Quit() } catch { }
    exit 1
} finally {
    try { $app.Quit() } catch { }
    try { [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($app) } catch { }
}

if (-not (Test-Path $Dst)) {
    [Console]::Error.WriteLine("the application reported success but wrote no file")
    exit 1
}
exit 0
