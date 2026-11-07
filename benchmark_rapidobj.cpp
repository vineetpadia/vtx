#include <rapidobj/rapidobj.hpp>
#include <chrono>
#include <fstream>
#include <iostream>
int main(int argc, char** argv) {
    if (argc < 2 || argc > 3) {
        std::cerr << "Usage: rapid_bench input.obj [packed-triangles.bin]\n";
        return 2;
    }
    using Clock = std::chrono::steady_clock;
    using Ms = std::chrono::duration<double, std::milli>;
    const auto start = Clock::now();
    auto result = rapidobj::ParseFile(argv[1]);
    auto parsed = Clock::now();
    if (result.error) { std::cerr << result.error.code.message() << '\n'; return 1; }
    if (!rapidobj::Triangulate(result)) { std::cerr << "Triangulation failed\n"; return 1; }
    auto finished = Clock::now();
    size_t count = 0;
    for (const auto& shape : result.shapes) count += shape.mesh.indices.size() / 3;
    std::cout << "parse_ms=" << Ms(parsed-start).count() << " mesh_ms=" << Ms(finished-parsed).count()
              << " total_ms=" << Ms(finished-start).count() << " triangles=" << count << '\n';
    if (argc > 2) {
        std::ofstream out(argv[2], std::ios::binary);
        for (const auto& shape : result.shapes) for (const auto& corner : shape.mesh.indices)
            out.write(reinterpret_cast<const char*>(result.attributes.positions.data() + 3 * corner.position_index), 12);
        if (!out) return 2;
    }
}

