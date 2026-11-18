# Launch the isolated pressure test canvas from any working directory.
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
Set-Location $repo
cargo run -p nocturne --example pressure_canvas
