// Build against FastGLTF and its simdjson/simdutf dependencies.
#include <fastgltf/core.hpp>
#include <fastgltf/tools.hpp>
#include <array>
#include <chrono>
#include <cmath>
#include <fstream>
#include <iostream>
#include <stdexcept>
#include <vector>

struct Primitive {
    std::vector<fastgltf::math::fvec3> points;
    std::vector<std::uint32_t> indices;
};

int main(int argc, char** argv) try {
    if (argc < 2 || argc > 3) {
        std::cerr << "Usage: fastgltf_bench input.glb [packed-triangles.bin]\n";
        return 2;
    }
    using Clock = std::chrono::steady_clock;
    using Ms = std::chrono::duration<double, std::milli>;
    const auto start = Clock::now();
    const auto path = std::filesystem::path(argv[1]);
    auto input = fastgltf::GltfDataBuffer::FromPath(path);
    if (input.error() != fastgltf::Error::None) throw std::runtime_error("Cannot open glTF");
    fastgltf::Parser parser;
    auto asset = parser.loadGltf(input.get(), path.parent_path(), fastgltf::Options::LoadExternalBuffers);
    if (asset.error() != fastgltf::Error::None) {
        throw std::runtime_error(std::string(fastgltf::getErrorMessage(asset.error())));
    }
    auto& model = asset.get();
    const auto parsed = Clock::now();
    if (model.scenes.empty()) throw std::runtime_error("No glTF scene");
    std::vector<std::optional<std::vector<Primitive>>> meshes(model.meshes.size());
    std::vector<std::pair<std::size_t, fastgltf::math::nmat4x4>> instances;
    std::size_t triangles = 0;
    fastgltf::iterateSceneNodes(model, model.defaultScene.value_or(0), fastgltf::math::nmat4x4(),
        [&](fastgltf::Node& node, fastgltf::math::nmat4x4 world) {
            if (!node.meshIndex.has_value()) return;
            auto id = *node.meshIndex;
            if (!meshes[id]) {
                auto& decoded = meshes[id].emplace();
                for (const auto& source : model.meshes[id].primitives) {
                    if (source.type != fastgltf::PrimitiveType::Triangles) {
                        throw std::runtime_error("Benchmark requires triangle-list primitives");
                    }
                    const auto pos = source.findAttribute("POSITION");
                    if (pos == source.attributes.end()) throw std::runtime_error("Missing positions");
                    const auto& positions = model.accessors[pos->accessorIndex];
                    auto& primitive = decoded.emplace_back();
                    primitive.points.resize(positions.count);
                    fastgltf::copyFromAccessor<fastgltf::math::fvec3>(model, positions, primitive.points.data());
                    for (const auto& p : primitive.points) {
                        if (!std::isfinite(p[0]) || !std::isfinite(p[1]) || !std::isfinite(p[2])) {
                            throw std::runtime_error("Non-finite position");
                        }
                    }
                    if (source.indicesAccessor.has_value()) {
                        const auto& indices = model.accessors[*source.indicesAccessor];
                        primitive.indices.resize(indices.count);
                        fastgltf::copyFromAccessor<std::uint32_t>(model, indices, primitive.indices.data());
                    } else {
                        primitive.indices.resize(positions.count);
                        for (std::size_t i = 0; i < positions.count; i++) primitive.indices[i] = std::uint32_t(i);
                    }
                    if (primitive.indices.size() % 3) throw std::runtime_error("Incomplete triangle");
                    for (auto i : primitive.indices) {
                        if (i >= primitive.points.size()) throw std::runtime_error("Invalid index");
                    }
                }
            }
            for (const auto& primitive : *meshes[id]) triangles += primitive.indices.size() / 3;
            instances.emplace_back(id, world);
        });
    const auto ready = Clock::now();
    std::cout << "parse_ms=" << Ms(parsed-start).count() << " mesh_ms=" << Ms(ready-parsed).count()
              << " total_ms=" << Ms(ready-start).count() << " triangles=" << triangles << '\n';
    if (argc == 3) {
        std::ofstream out(argv[2], std::ios::binary);
        for (const auto& [id, world] : instances) {
            const auto a = world[0], b = world[1], c = world[2];
            const bool mirrored = a[0]*(b[1]*c[2]-b[2]*c[1]) -
                a[1]*(b[0]*c[2]-b[2]*c[0]) + a[2]*(b[0]*c[1]-b[1]*c[0]) < 0;
            for (const auto& primitive : *meshes[id]) {
                for (std::size_t i = 0; i < primitive.indices.size(); i += 3) {
                    for (std::size_t j : {0u, mirrored ? 2u : 1u, mirrored ? 1u : 2u}) {
                        const auto p = primitive.points[primitive.indices[i+j]];
                        const auto point = world * fastgltf::math::fvec4(p[0],p[1],p[2],1.0f);
                        const std::array<float,3> position{point[0],point[1],point[2]};
                        out.write(reinterpret_cast<const char*>(position.data()), sizeof(position));
                    }
                }
            }
        }
        if (!out) return 2;
    }
    return 0;
} catch (const std::exception& error) {
    std::cerr << error.what() << '\n';
    return 1;
}
