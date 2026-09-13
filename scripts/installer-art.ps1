# Reproducible local NSIS artwork. No network fonts or remote assets.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$assetDir = Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri/installer'
New-Item -ItemType Directory -Force -Path $assetDir | Out-Null
$icon = [Drawing.Image]::FromFile((Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri/icons/128x128.png'))
foreach ($kind in @('sidebar','header')) {
  $w = if ($kind -eq 'sidebar') {164} else {150}
  $h = if ($kind -eq 'sidebar') {314} else {57}
  $bmp = [Drawing.Bitmap]::new($w,$h)
  $g = [Drawing.Graphics]::FromImage($bmp)
  $g.Clear([Drawing.ColorTranslator]::FromHtml('#191c20'))
  $g.SmoothingMode = 'AntiAlias'
  $ink = [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#f0ede6'))
  $muted = [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#8da9bf'))
  $font = [Drawing.Font]::new('Segoe UI',20,[Drawing.FontStyle]::Regular)
  $small = [Drawing.Font]::new('Segoe UI',9)
  if ($kind -eq 'sidebar') {
    $g.DrawImage($icon,18,28,72,72)
    $g.DrawString('Pulse',$font,$ink,20,112)
    $g.DrawString("Seu Windows,`nem perspectiva.",$small,$muted,22,156)
    $pen = [Drawing.Pen]::new([Drawing.ColorTranslator]::FromHtml('#343a42'),1)
    for ($i=0; $i -lt 12; $i++) { $g.DrawLine($pen,22,220+$i*5,142-($i%3)*12,220+$i*5) }
    $g.DrawString('LOCAL. SEU.',$small,$muted,22,288)
    $pen.Dispose()
  } else {
    $g.DrawImage($icon,4,5,46,46)
    $g.DrawString('Pulse',$font,$ink,54,9)
  }
  $bmp.Save((Join-Path $assetDir "$kind.bmp"),[Drawing.Imaging.ImageFormat]::Bmp)
  $g.Dispose(); $bmp.Dispose(); $font.Dispose(); $small.Dispose(); $ink.Dispose(); $muted.Dispose()
}
$icon.Dispose()
