# SolidWorks display mesh reader

The vendored modules are from [XRTC5/sldprt-export](https://github.com/XRTC5/sldprt-export/tree/31d3b5a9b115d9cbfc19bbf10906fcc29e8e3c57). See the included MIT license.

VTX added bounded, CRC-checked decompression and support for resolving explicitly supplied parts when an assembly has an instance tree but lacks a cached mesh directory. The upstream reader credits [cadmpeg](https://github.com/cadmpeg/cadmpeg) for its description of the container layout.

The standalone Rust importer uses the same container, strip, and assembly layouts.
