$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$sourcePath = Join-Path $PSScriptRoot '..\ui-demo\assets\firenet-48.png'
$iconPath = Join-Path $PSScriptRoot '..\ui-demo\assets\firenet.ico'
$rgbaPath = Join-Path $PSScriptRoot '..\ui-demo\assets\firenet-48.rgba'
$source = [System.Drawing.Bitmap]::FromFile($sourcePath)
try {
    if ($source.Width -ne 48 -or $source.Height -ne 48) { throw 'Expected a 48x48 source PNG' }

    $rgba = New-Object byte[] ($source.Width * $source.Height * 4)
    for ($y = 0; $y -lt 48; $y++) {
        for ($x = 0; $x -lt 48; $x++) {
            $pixel = $source.GetPixel($x, $y)
            $offset = ($y * 48 + $x) * 4
            $rgba[$offset] = $pixel.R
            $rgba[$offset + 1] = $pixel.G
            $rgba[$offset + 2] = $pixel.B
            $rgba[$offset + 3] = $pixel.A
        }
    }
    [System.IO.File]::WriteAllBytes($rgbaPath, $rgba)

    $images = @()
    foreach ($size in @(16, 32, 48, 256)) {
        if ($size -eq 48) {
            $images += ,([System.IO.File]::ReadAllBytes($sourcePath))
            continue
        }
        $bitmap = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        $stream = New-Object System.IO.MemoryStream
        try {
            $graphics.Clear([System.Drawing.Color]::Transparent)
            $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
            $graphics.DrawImage($source, 0, 0, $size, $size)
            $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
            $images += ,($stream.ToArray())
        } finally {
            $stream.Dispose()
            $graphics.Dispose()
            $bitmap.Dispose()
        }
    }

    $output = [System.IO.File]::Create($iconPath)
    $writer = New-Object System.IO.BinaryWriter($output)
    try {
        $writer.Write([uint16]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]$images.Count)
        $offset = 6 + $images.Count * 16
        for ($index = 0; $index -lt $images.Count; $index++) {
            $size = @(16, 32, 48, 256)[$index]
            $writer.Write([byte]($size % 256))
            $writer.Write([byte]($size % 256))
            $writer.Write([byte]0)
            $writer.Write([byte]0)
            $writer.Write([uint16]1)
            $writer.Write([uint16]32)
            $writer.Write([uint32]$images[$index].Length)
            $writer.Write([uint32]$offset)
            $offset += $images[$index].Length
        }
        foreach ($data in $images) { $writer.Write([byte[]]$data) }
    } finally {
        $writer.Dispose()
    }
} finally {
    $source.Dispose()
}
