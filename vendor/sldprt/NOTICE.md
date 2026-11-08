# SolidWorks display mesh reader

Imported-body appearance matching also uses the documented LocalBodies framed Parasolid chains and owned `ATOM_ID_2001` / `SDL/TYSA_NAME` attributes. The implementation reads only bounded identity data, rejects conflicting owners, and matches named display-body groups. Reused appearance objects are resolved through a conservative archive-object census. The headstage's 27 colored body appearances, including all 17 blue logo bodies and the cream label stock, were checked against the local SolidWorks API.

The projection-decal reader uses the section-8 decal/image layouts as a starting point, with an additional modern DisplayLists layout checked against locally created native SolidWorks decal metadata. Only a complete placement, unique face trailer, repeated persistent references, and embedded/sibling image can bind a decal. GPU compositing preserves base material outside the image and under PNG alpha. Native projection screen parity was not established; all image decals therefore retain the saved Explorer image. The diagnostic/no-thumbnail renderer can use the limited projection decoder. This is a limited read-only display implementation, not full decal or material equivalence.

The native reader also applies supported saved feature appearances through matching source IDs, Keywords owners, resolved-feature timestamps, and enabled VisualStates attributes. Named `.p2m` appearance RGB fields and supported single display-body scopes supplement the legacy packed color. These additions were checked against the local SolidWorks API for the Pomona corner parts. They do not provide complete rendering equivalence for arbitrary appearances, display states, textures, or decals.

For top-level parts in a placement-only assembly, the reader can use a configured part's closed saved visible mesh when every quantized triangle is an exact subset of the assembly cache. This avoids drawing extra hidden-body geometry retained by that cache. Unknown assembly features, nested contexts, open displays, and changed triangle geometry keep the assembly cache. The Pomona regression was checked against SolidWorks' body visibility values: both compression parts hide their `Mirror1` body.

The vendored modules are from [XRTC5/sldprt-export](https://github.com/XRTC5/sldprt-export/tree/31d3b5a9b115d9cbfc19bbf10906fcc29e8e3c57). See the included MIT license.

VTX added bounded, CRC-checked decompression and support for resolving explicitly supplied parts when an assembly has an instance tree but lacks a cached mesh directory. The upstream reader credits [cadmpeg](https://github.com/cadmpeg/cadmpeg) for its description of the container layout.

The standalone Rust importer uses the same container, strip, and assembly layouts.

The standalone RGB appearance reader also uses the published visual-property, DisplayLists assignment, and VisualStates part-scope layouts in [cadmpeg's SolidWorks format notes, section 8](https://github.com/cadmpeg/cadmpeg/blob/main/docs/formats/sldprt.md), licensed under CC BY 4.0. The implementation adds bounded appearance validation, rejects ambiguous tags and display states, and sends the supported colors to the GPU viewer. It does not reconstruct all SolidWorks appearance semantics.
