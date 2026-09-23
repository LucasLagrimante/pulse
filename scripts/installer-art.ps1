# Arte NSIS com a identidade Pulse (grafite Obsidian + âmbar).
# Pipeline canônico (ordem importa — o ícone vem do SVG novo):
#   1. npx tauri icon public/pulse.svg
#   2. powershell ./scripts/installer-art.ps1
#   3. npm run tauri:build
# Sem fontes de rede nem assets remotos. Tamanhos exigidos pelo NSIS MUI:
# sidebar 164x314 (welcome/finish), header 150x57 (topo das páginas).
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
  $g.Clear([Drawing.ColorTranslator]::FromHtml('#181b20'))
  $g.SmoothingMode = 'AntiAlias'
  $ink = [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#f1f5f9'))
  $muted = [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#8da9bf'))
  $accent = [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#f59e0b'))
  $accentPen = [Drawing.Pen]::new([Drawing.ColorTranslator]::FromHtml('#f59e0b'),2)
  $font = [Drawing.Font]::new('Segoe UI',20,[Drawing.FontStyle]::Regular)
  $small = [Drawing.Font]::new('Segoe UI',9)
  if ($kind -eq 'sidebar') {
    $g.DrawImage($icon,18,28,72,72)
    $g.DrawString('Pulse',$font,$ink,20,112)
    $g.FillRectangle($accent,22,150,38,3)
    $g.DrawString("Seu Windows,`nem perspectiva.",$small,$muted,22,160)
    # Onda do pulso em âmbar — ecoa o símbolo de public/pulse.svg.
    $wave = @(
      [Drawing.Point]::new(22,236),[Drawing.Point]::new(52,236),
      [Drawing.Point]::new(62,236),[Drawing.Point]::new(70,206),
      [Drawing.Point]::new(78,258),[Drawing.Point]::new(86,224),
      [Drawing.Point]::new(94,236),[Drawing.Point]::new(142,236)
    )
    $g.DrawLines($accentPen,$wave)
    $g.DrawString('LOCAL. SEU.',$small,$muted,22,288)
  } else {
    $g.DrawImage($icon,4,5,46,46)
    $g.DrawString('Pulse',$font,$ink,54,9)
    # Filete âmbar na borda esquerda separa o header da página clara do NSIS.
    $g.FillRectangle($accent,0,0,2,57)
  }
  $bmp.Save((Join-Path $assetDir "$kind.bmp"),[Drawing.Imaging.ImageFormat]::Bmp)
  $g.Dispose(); $bmp.Dispose(); $font.Dispose(); $small.Dispose()
  $ink.Dispose(); $muted.Dispose(); $accent.Dispose(); $accentPen.Dispose()
}
$icon.Dispose()
