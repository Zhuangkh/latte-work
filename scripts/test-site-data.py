#!/usr/bin/env python3
"""Verify site-data boundaries using a real, disposable WebKit cookie store."""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='latte-site-data-') as directory:
    binary = str(Path(directory) / 'test-site-data')
    subprocess.run(['xcrun', 'clang', '-fobjc-arc', '-Wall', '-Wextra', '-Werror',
                    '-Wno-unused-parameter', '-framework', 'WebKit', '-framework', 'AppKit',
                    str(root / 'apps/desktop/src-tauri/tests/macos_site_data.m'), '-o', binary], check=True)
    subprocess.run([binary], check=True, timeout=60)
