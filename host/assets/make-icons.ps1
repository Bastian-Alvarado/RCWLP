# Draws the RCWLP icon and writes rcwlp.ico, 16-256 px, for the programs, their tray icon and the
# installer. It is SimonSays's logo tile with a gamepad in its hexagon: white lines on a rose
# gradient, the same mark as the phone app's (android/app/src/main/res/drawable/logo_tile.xml).
# Below 32 px, where the hexagon would blur, the tile holds just a larger gamepad.
# Also writes the installer's wizard images, installer/wizard-*.png.
# Run from this folder:  powershell -ExecutionPolicy Bypass -File make-icons.ps1
Add-Type -AssemblyName System.Drawing

# Adds an SVG-style circular arc from the path's last point to ($x, $y) with radius $r.
function Add-Arc($path, [double]$x1, [double]$y1, [double]$r, [bool]$large, [bool]$sweep, [double]$x2, [double]$y2) {
    $hx = ($x1 - $x2) / 2; $hy = ($y1 - $y2) / 2
    $d2 = $hx * $hx + $hy * $hy
    $k = [Math]::Sqrt([Math]::Max(0, ($r * $r - $d2) / $d2))
    if ($large -eq $sweep) { $k = -$k }
    $cx = $k * $hy + ($x1 + $x2) / 2; $cy = -$k * $hx + ($y1 + $y2) / 2
    $a1 = [Math]::Atan2($y1 - $cy, $x1 - $cx); $a2 = [Math]::Atan2($y2 - $cy, $x2 - $cx)
    $da = $a2 - $a1
    if ($sweep -and $da -lt 0) { $da += 2 * [Math]::PI }
    if (-not $sweep -and $da -gt 0) { $da -= 2 * [Math]::PI }
    $path.AddArc([float]($cx - $r), [float]($cy - $r), [float](2 * $r), [float](2 * $r), [float]($a1 * 180 / [Math]::PI), [float]($da * 180 / [Math]::PI))
}

# The mark on a 24-unit grid, as in the app's vector drawables.
function Mark {
    $hex = New-Object System.Drawing.Drawing2D.GraphicsPath
    $hex.AddPolygon([System.Drawing.PointF[]]@(
        (New-Object System.Drawing.PointF 12, 2), (New-Object System.Drawing.PointF 21, 7), (New-Object System.Drawing.PointF 21, 17),
        (New-Object System.Drawing.PointF 12, 22), (New-Object System.Drawing.PointF 3, 17), (New-Object System.Drawing.PointF 3, 7)))
    $pad = New-Object System.Drawing.Drawing2D.GraphicsPath
    $pad.AddLine(9.5, 8.9, 14.5, 8.9)
    Add-Arc $pad 14.5 8.9 3 $false $true 17.45 11.35
    $pad.AddLine(17.45, 11.35, 17.9, 13.8)
    Add-Arc $pad 17.9 13.8 1.6 $false $true 15.15 15.2
    $pad.AddLine(15.15, 15.2, 14.1, 13.9)
    $pad.AddLine(14.1, 13.9, 9.9, 13.9)
    $pad.AddLine(9.9, 13.9, 8.85, 15.2)
    Add-Arc $pad 8.85 15.2 1.6 $false $true 6.1 13.8
    $pad.AddLine(6.1, 13.8, 6.55, 11.35)
    Add-Arc $pad 6.55 11.35 3 $false $true 9.5 8.9
    $pad.CloseFigure()
    return $hex, $pad
}

function Draw([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)

    # The tile: rose-500 to rose-600 towards the top right, corners a quarter of its size.
    $s = [float]$size
    $tile = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $s / 2
    $tile.AddArc(0, 0, $d, $d, 180, 90); $tile.AddArc($s - $d, 0, $d, $d, 270, 90)
    $tile.AddArc($s - $d, $s - $d, $d, $d, 0, 90); $tile.AddArc(0, $s - $d, $d, $d, 90, 90)
    $tile.CloseFigure()
    $rose = [System.Drawing.Color]::FromArgb(255, 0xF4, 0x3F, 0x5E)
    $roseDark = [System.Drawing.Color]::FromArgb(255, 0xE1, 0x1D, 0x48)
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush ((New-Object System.Drawing.PointF 0, $s), (New-Object System.Drawing.PointF $s, 0), $rose, $roseDark)
    $g.FillPath($brush, $tile)

    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::White), 2
    $pen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
    $pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $hex, $pad = Mark
    if ($size -ge 32) {
        # The mark fills 62.5% of the tile, as in SimonSays.
        $scale = $s * 0.625 / 24
        $g.TranslateTransform($s * 0.375 / 2, $s * 0.375 / 2)
        $g.ScaleTransform($scale, $scale)
        $g.DrawPath($pen, $hex)
    } else {
        # The gamepad alone, 76% of the tile wide, centred.
        $scale = $s * 0.76 / 11.8
        $g.TranslateTransform($s / 2, $s / 2)
        $g.ScaleTransform($scale, $scale)
        $g.TranslateTransform(-12, -12.05)
    }
    $g.DrawPath($pen, $pad)
    $white = [System.Drawing.Brushes]::White
    $g.FillEllipse($white, 8.65, 10.65, 1.5, 1.5)
    $g.FillEllipse($white, 13.85, 10.65, 1.5, 1.5)
    $g.Dispose()
    return $bmp
}

function PngBytes([System.Drawing.Bitmap]$bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    return , $ms.ToArray()
}

$here = Split-Path -Parent $MyInvocation.MyCommand.Path

# An .ico holding one PNG per size (the format Windows has accepted since Vista).
$sizes = 16, 20, 24, 32, 48, 64, 128, 256
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
"wrote rcwlp.ico ($($sizes -join ', ') px)"

# The installer's side image, laid out like SimonSays's splash screen: the logo tile and the
# wordmark on near-black with a faint rose glow, and a short rose bar.
$fonts = New-Object System.Drawing.Text.PrivateFontCollection
$fonts.AddFontFile((Join-Path $here "fonts\montserrat_black.ttf"))

function Side([int]$width, [int]$height) {
    $bmp = New-Object System.Drawing.Bitmap $width, $height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAlias
    $g.Clear([System.Drawing.Color]::FromArgb(255, 5, 5, 5))
    $u = $width / 164.0
    $glow = New-Object System.Drawing.Drawing2D.GraphicsPath
    $glow.AddEllipse([float](-60 * $u), [float](10 * $u), [float](284 * $u), [float](284 * $u))
    $brush = New-Object System.Drawing.Drawing2D.PathGradientBrush $glow
    $brush.CenterColor = [System.Drawing.Color]::FromArgb(56, 0xF4, 0x3F, 0x5E)
    $brush.SurroundColors = @([System.Drawing.Color]::FromArgb(0, 0xF4, 0x3F, 0x5E))
    $g.FillPath($brush, $glow)

    $tile = [int](60 * $u)
    $g.DrawImage((Draw $tile), [int](($width - $tile) / 2), [int](104 * $u), $tile, $tile)
    $font = New-Object System.Drawing.Font $fonts.Families[0], ([float](24 * $u)), ([System.Drawing.GraphicsUnit]::Pixel)
    $format = New-Object System.Drawing.StringFormat
    $format.Alignment = [System.Drawing.StringAlignment]::Center
    $g.DrawString("RCWLP", $font, [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF 0, ([float](178 * $u)), $width, ([float](40 * $u))), $format)
    $rose = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 0xF4, 0x3F, 0x5E))
    $g.FillRectangle($rose, [float](($width - 26 * $u) / 2), [float](214 * $u), [float](26 * $u), [float](3 * $u))
    $g.Dispose()
    return $bmp
}

# 100% and 200% sizes of each; Setup picks the best fit for the screen.
$installer = Join-Path (Split-Path -Parent (Split-Path -Parent $here)) "installer"
foreach ($scale in 1, 2) {
    (Side (164 * $scale) (314 * $scale)).Save((Join-Path $installer "wizard-$(164 * $scale).png"), [System.Drawing.Imaging.ImageFormat]::Png)
    (Draw (58 * $scale)).Save((Join-Path $installer "wizard-small-$(58 * $scale).png"), [System.Drawing.Imaging.ImageFormat]::Png)
}
"wrote the installer's wizard images"
