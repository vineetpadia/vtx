#ifndef VTX_IGES_BRIDGE_H
#define VTX_IGES_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#define VTX_IGES_EXPORT __declspec(dllexport)
#else
#define VTX_IGES_EXPORT __attribute__((visibility("default")))
#endif

#ifdef __cplusplus
extern "C" {
#endif

typedef struct VtxIgesMeshV1 {
    const float* positions;
    const uint32_t* indices;
    size_t vertices;
    size_t triangles;
    size_t skipped_faces;
    double read_ms;
    double transfer_ms;
    double mesh_ms;
    double extraction_ms;
    void* storage;
} VtxIgesMeshV1;

// Positions and indices are packed XYZ and oriented triangle triples. The
// caller owns the result until vtx_iges_free_v1. Errors leave it empty.
VTX_IGES_EXPORT int vtx_iges_load_v1(const char* path, double deflection, VtxIgesMeshV1* mesh,
                                    char* error, size_t error_size);
VTX_IGES_EXPORT void vtx_iges_free_v1(VtxIgesMeshV1* mesh);

#ifdef __cplusplus
}
#endif
#endif
