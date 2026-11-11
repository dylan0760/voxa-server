# Generates the WiX installer artwork (banner 493x58, dialog 493x312) from the icons.
param([string]$Assets = (Join-Path $PSScriptRoot "..\assets"))
Add-Type -AssemblyName System.Drawing

function New-Art([string]$ico, [string]$prefix, [string]$title, [System.Drawing.Color]$c1, [System.Drawing.Color]$c2) {
    $img = [System.Drawing.Image]::FromFile((Join-Path $Assets ($ico -replace "\.ico$", ".png")))

    # Banner: white with icon on the right (WiX draws the title text on the left).
    $b = New-Object System.Drawing.Bitmap 493, 58
    $g = [System.Drawing.Graphics]::FromImage($b)
    $g.SmoothingMode = 'HighQuality'; $g.InterpolationMode = 'HighQualityBicubic'
    $g.Clear([System.Drawing.Color]::White)
    $g.DrawImage($img, 493 - 52, 7, 44, 44)
    $b.Save((Join-Path $Assets "$prefix-banner.bmp"), [System.Drawing.Imaging.ImageFormat]::Bmp)
    $g.Dispose(); $b.Dispose()

    # Dialog: coloured panel on the left 164 px, white elsewhere.
    $d = New-Object System.Drawing.Bitmap 493, 312
    $g = [System.Drawing.Graphics]::FromImage($d)
    $g.SmoothingMode = 'HighQuality'; $g.InterpolationMode = 'HighQualityBicubic'; $g.TextRenderingHint = 'AntiAliasGridFit'
    $g.Clear([System.Drawing.Color]::White)
    $rect = New-Object System.Drawing.Rectangle 0, 0, 164, 312
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush($rect, $c1, $c2, 90)
    $g.FillRectangle($brush, $rect)
    $g.DrawImage($img, 32, 70, 100, 100)
    $font = New-Object System.Drawing.Font("Segoe UI", 16, [System.Drawing.FontStyle]::Bold)
    $sf = New-Object System.Drawing.StringFormat; $sf.Alignment = 'Center'
    $g.DrawString($title, $font, [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF 0, 185, 164, 40), $sf)
    $small = New-Object System.Drawing.Font("Segoe UI", 8)
    $g.DrawString("Free & open source", $small, [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF 0, 220, 164, 20), $sf)
    $d.Save((Join-Path $Assets "$prefix-dialog.bmp"), [System.Drawing.Imaging.ImageFormat]::Bmp)
    $g.Dispose(); $d.Dispose()
}

New-Art "voxa-server.ico" "server" "Voxa Server" ([System.Drawing.Color]::FromArgb(40, 160, 95)) ([System.Drawing.Color]::FromArgb(20, 80, 50))
"installer artwork written to $Assets"
