# Draws the RCWLP icon (a white gamepad on the app's navy tile) and writes:
#   tray.png   64 px, for the tray icon
#   rcwlp.ico  16-256 px, for the programs and the installer
# Run from this folder:  powershell -ExecutionPolicy Bypass -File make-icons.ps1
Add-Type -AssemblyName System.Drawing

function Draw([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.ScaleTransform($size / 64.0, $size / 64.0)
    $navy = [System.Drawing.Color]::FromArgb(255, 0x1E, 0x2A, 0x44)

    function RoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
        $p = New-Object System.Drawing.Drawing2D.GraphicsPath
        $p.AddArc($x, $y, 2 * $r, 2 * $r, 180, 90)
        $p.AddArc($x + $w - 2 * $r, $y, 2 * $r, 2 * $r, 270, 90)
        $p.AddArc($x + $w - 2 * $r, $y + $h - 2 * $r, 2 * $r, 2 * $r, 0, 90)
        $p.AddArc($x, $y + $h - 2 * $r, 2 * $r, 2 * $r, 90, 90)
        $p.CloseFigure()
        return $p
    }

    # Background tile, like the app's adaptive icon.
    $g.FillPath((New-Object System.Drawing.SolidBrush $navy), (RoundRect 0 0 64 64 14))
    # Gamepad body with two grips.
    $body = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
    $g.FillPath($body, (RoundRect 8 19 48 22 11))
    $g.FillEllipse($body, 7, 28, 17, 21)
    $g.FillEllipse($body, 40, 28, 17, 21)
    # D-pad and two face buttons.
    $mark = New-Object System.Drawing.SolidBrush $navy
    $g.FillRectangle($mark, 15, 28, 11, 4)
    $g.FillRectangle($mark, 18.5, 24.5, 4, 11)
    $g.FillEllipse($mark, 38, 24, 6, 6)
    $g.FillEllipse($mark, 44, 30, 6, 6)
    $g.Dispose()
    return $bmp
}

function PngBytes([System.Drawing.Bitmap]$bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    return , $ms.ToArray()
}

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
(Draw 64).Save((Join-Path $here "tray.png"), [System.Drawing.Imaging.ImageFormat]::Png)

# An .ico holding one PNG per size (the format Windows has accepted since Vista).
$sizes = 16, 24, 32, 48, 64, 128, 256
$images = foreach ($s in $sizes) { , (PngBytes (Draw $s)) }
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]
    $dim = if ($s -ge 256) { 0 } else { $s }  # 0 means 256
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$images[$i].Length); $w.Write([uint32]$offset)
    $offset += $images[$i].Length
}
foreach ($img in $images) { $w.Write($img) }
$w.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $here "rcwlp.ico"), $out.ToArray())
"wrote tray.png and rcwlp.ico ($($sizes -join ', ') px)"
