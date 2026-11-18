@echo off
setlocal
cd /d "%~dp0"
REM Enable debug logging for UI egui (container rendering, image resize, etc)
set RUST_LOG=nocturne_ui_egui=debug
REM Pass optional Nocturne arguments through, for example: run.cmd --first-use
cargo run --release -p nocturne -- --notebooks .\my-notebooks %*
