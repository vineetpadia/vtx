#include "ufbx.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

int main(int argc, char **argv) {
    if (argc != 4) return 2;
    const int lean = strcmp(argv[1], "geometry") == 0;
    ufbx_load_opts opts = {0};
    opts.ignore_animation = lean;
    opts.ignore_embedded = lean;
    ufbx_error error;
    clock_t start = clock();
    ufbx_scene *scene = ufbx_load_file(argv[2], &opts, &error);
    if (!scene) { fprintf(stderr, "%s\n", error.description.data); return 1; }
    fprintf(stderr, "parse_ms=%.3f temp=%zu scene=%zu\n",
        (double)(clock() - start) * 1000 / CLOCKS_PER_SEC,
        scene->metadata.temp_memory_used, scene->metadata.result_memory_used);
    FILE *out = fopen(argv[3], "wb");
    if (!out) return 1;
    for (size_t i = 0; i < scene->nodes.count; i++) {
        ufbx_node *node = scene->nodes.data[i];
        ufbx_mesh *mesh = node->mesh;
        if (!mesh || !mesh->max_face_triangles) continue;
        uint32_t *corners = malloc(mesh->max_face_triangles * 3 * sizeof(uint32_t));
        if (!corners) return 1;
        int mirrored = ufbx_matrix_determinant(&node->geometry_to_world) < 0;
        for (size_t face = 0; face < mesh->faces.count; face++) {
            uint32_t n = ufbx_triangulate_face(corners, mesh->max_face_triangles * 3, mesh, mesh->faces.data[face]);
            for (size_t t = 0; t < n; t++) {
                for (size_t k = 0; k < 3; k++) {
                    size_t c = mirrored && k ? 3 - k : k;
                    ufbx_vec3 p = ufbx_get_vertex_vec3(&mesh->vertex_position, corners[t * 3 + c]);
                    p = ufbx_transform_position(&node->geometry_to_world, p);
                    float data[] = {(float)p.x, (float)p.y, (float)p.z};
                    if (fwrite(data, sizeof(data), 1, out) != 1) return 1;
                }
            }
        }
        free(corners);
    }
    ufbx_free_scene(scene);
    return fclose(out) != 0;
}
