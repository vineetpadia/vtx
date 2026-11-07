#define TINYPLY_IMPLEMENTATION
#include "tinyply.h"
#include "example-utils.hpp"

#include <algorithm>
#include <chrono>
#include <fstream>
#include <iostream>

int main(int argc, char** argv) {
    if (argc < 2 || argc > 3) {
        std::cerr << "Usage: tinyply_bench input.ply [output-prefix]\n";
        return 2;
    }
    try {
        using Clock = std::chrono::steady_clock;
        const auto start = Clock::now();
        const auto bytes = read_file_binary(argv[1]);
        if (bytes.empty()) throw std::runtime_error("Empty input");
        memory_stream source(reinterpret_cast<const char*>(bytes.data()), bytes.size());
        tinyply::PlyFile file;
        if (!file.parse_header(source)) throw std::runtime_error("Invalid PLY header");

        const auto positions = file.request_properties_from_element("vertex", {"x", "y", "z"});
        std::shared_ptr<tinyply::PlyData> colors, indices;
        for (const auto& element : file.get_elements()) {
            if (element.name == "vertex" && std::any_of(element.properties.begin(), element.properties.end(),
                    [](const auto& property) { return property.name == "red"; })) {
                colors = file.request_properties_from_element("vertex", {"red", "green", "blue"});
            }
            if (element.name == "face" && element.size) {
                indices = file.request_properties_from_element("face", {"vertex_indices"}, 3);
            }
        }
        file.read(source);
        const auto elapsed = std::chrono::duration<double, std::milli>(Clock::now() - start).count();
        std::cout << "total_ms=" << elapsed << " triangles=" << (indices ? indices->count : 0)
                  << " vertices=" << positions->count << '\n';

        if (argc == 3) {
            const auto write = [&](const char* suffix, const std::shared_ptr<tinyply::PlyData>& data) {
                if (!data) return;
                std::ofstream output(std::string(argv[2]) + suffix, std::ios::binary);
                output.write(reinterpret_cast<const char*>(data->buffer.get_const()), data->buffer.size_bytes());
                if (!output) throw std::runtime_error("Cannot write PLY comparison output");
            };
            write(".xyz", positions);
            write(".rgb", colors);
            write(".indices", indices);
        }
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
