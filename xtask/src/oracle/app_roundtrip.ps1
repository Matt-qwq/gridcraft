# Open a GridCraft-written workbook in an independent spreadsheet application, recalculate it and
# save a copy, so `cargo xtask oracle` can read the copy back and compare it with the model.
#
# `-App` is the COM ProgID: Excel.Application (the gate check) or KET.Application (WPS, the
# substitute used while Excel was unavailable). Exit codes: 0 = a copy was written, 2 = the
# application is not installed, 1 = it refused or failed.
param(
    [Parameter(Mandatory=$true)][string]$Src,
    [Parameter(Mandatory=$true)][string]$Dst,
    [Parameter(Mandatory=$true)][string]$ProgId
)

$ErrorActionPreference = 'Stop'

try {
    $application = New-Object -ComObject $ProgId
} catch {
    [Console]::Error.WriteLine("the application is not installed (no $ProgId)")
    exit 2
}

try {
    $application.Visible = $false
    $application.DisplayAlerts = $false
    try { $application.AskToUpdateLinks = $false } catch { }
    try { $application.CalculateBeforeSave = $true } catch { }
    if (Test-Path $Dst) { Remove-Item $Dst -Force }
    # Open with the normal load: a file that needs repair must go through Excel's repair path so
    # the content comparison below can catch what was lost.
    $wb = $application.Workbooks.Open($Src, 0, $false)
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
    try { $application.Quit() } catch { }
    exit 1
} finally {
    try { $application.Quit() } catch { }
    try { [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($application) } catch { }
}

if (-not (Test-Path $Dst)) {
    [Console]::Error.WriteLine("the application reported success but wrote no file")
    exit 1
}
exit 0
