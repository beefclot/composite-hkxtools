# Composite HKX Conversion Tool

A Windows GUI that wraps hkxcmd, serde-hkx, and hkxconv for converting in between SSE HKX, LE, HKX, XML, and KF for behavior/animation modding.

I forked HKX Conversion Tool after opening a bugfix PR to serde-hkx, because I was making sure my upcoming behavior mod was working and properly creating patch files. It evolved from just adding hkxcmd to the app to also including hkxconv, since some people were asking for kf handling in the other page.

## Features

- SSE HKX, LE HKX, XML, and KF Conversion
- Batch conversion support
- User-friendlier GUI interface
- Specify output folder, file extension, and suffix options

## Installation

1. Download the latest release.
2. Extract the zip file to your desired location.
3. Run `composite-hkx-conversion.exe` file.

## Usage

1. Launch the application.
2. Select the convert tool you want to use at the top (hkxcmd, hkxc, or hkxconv)
3. OPTIONAL: If using hkxcmd you can convert using from or to KF.
4. Select whatever input files you want to handle/convert (specific files or entire folders/subfolders)
5. OPTIONAL: Select output folder or use same location as input file locations.
6. OPTIONAL: Set suffix to append with leading '_' to converted filenames.
7. OPTIONAL: Set override file extension for converted files.
8. Select converted Output Format.
9. Click 'Run Conversion' at bottom of window (might have to expand

## License

This project is licensed under the MIT License - see below for details:

MIT License

Copyright (c) 2023

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Credits

- hkxc.exe: This project uses a custom compiled CLI hkxc.exe based on [serde-hkx](https://www.nexusmods.com/skyrimspecialedition/mods/126214/) by SARDONYX.
- hkxconv.exe: This project uses the CLI hkxconv.exe from [hkxconv﻿](https://github.com/ret2end/hkxconv/tree/master) by ret2end.
- hkxcmd.exe: This project uses 1.5 hkxcmd, originally from [here](https://github.com/figment/hkxcmd)
- HavokContentTools
- HavokBehaviorPostProcess.exe