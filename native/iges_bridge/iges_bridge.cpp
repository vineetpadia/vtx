#include "iges_bridge.h"

#include <BRepMesh_IncrementalMesh.hxx>
#include <BRep_Tool.hxx>
#include <IGESControl_Reader.hxx>
#include <Message.hxx>
#include <Message_Messenger.hxx>
#include <Message_PrinterOStream.hxx>
#include <Poly_Triangulation.hxx>
#include <Standard_Failure.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Face.hxx>
#include <TopLoc_Location.hxx>
#include <gp_Trsf.hxx>

#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <exception>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <vector>

namespace {
using Point = std::array<float, 3>;
using Triangle = std::array<uint32_t, 3>;
using Clock = std::chrono::steady_clock;
static_assert(sizeof(Point) == 3 * sizeof(float));
static_assert(sizeof(Triangle) == 3 * sizeof(uint32_t));

constexpr size_t max_records = 16'000'000;

struct FaceMesh {
    Handle(Poly_Triangulation) mesh;
    TopLoc_Location location;
    bool reversed;
};

struct Storage {
    std::vector<Point> positions;
    std::vector<Triangle> triangles;
};

double elapsed_ms(Clock::time_point from, Clock::time_point to) {
    return std::chrono::duration<double, std::milli>(to - from).count();
}

std::unique_ptr<Storage> extract(const TopoDS_Shape& shape, size_t& skipped) {
    std::vector<FaceMesh> faces;
    size_t vertices = 0;
    size_t triangles = 0;
    for (TopExp_Explorer explorer(shape, TopAbs_FACE); explorer.More(); explorer.Next()) {
        const auto& face = TopoDS::Face(explorer.Current());
        TopLoc_Location location;
        const auto& mesh = BRep_Tool::Triangulation(face, location);
        if (mesh.IsNull() || mesh->NbTriangles() == 0) {
            ++skipped;
            continue;
        }
        vertices += static_cast<size_t>(mesh->NbNodes());
        triangles += static_cast<size_t>(mesh->NbTriangles());
        if (vertices > max_records || triangles > max_records) {
            throw std::runtime_error("IGES mesh exceeds the geometry limit");
        }
        faces.push_back({mesh, location, face.Orientation() == TopAbs_REVERSED});
    }
    if (triangles == 0) {
        throw std::runtime_error("IGES file contains no tessellated faces");
    }
    auto storage = std::make_unique<Storage>();
    storage->positions.reserve(vertices);
    storage->triangles.reserve(triangles);
    for (const auto& face : faces) {
        const auto& mesh = face.mesh;
        const bool placed = !face.location.IsIdentity();
        const auto& transform = face.location.Transformation();
        const auto base = static_cast<uint32_t>(storage->positions.size());
        const int nodes = mesh->NbNodes();
        for (int i = 1; i <= nodes; ++i) {
            auto point = mesh->Node(i);
            if (placed) {
                point.Transform(transform);
            }
            const Point xyz = {static_cast<float>(point.X()), static_cast<float>(point.Y()),
                               static_cast<float>(point.Z())};
            if (!std::isfinite(xyz[0]) || !std::isfinite(xyz[1]) || !std::isfinite(xyz[2])) {
                throw std::runtime_error("Invalid IGES vertex position");
            }
            storage->positions.push_back(xyz);
        }
        for (int i = 1; i <= mesh->NbTriangles(); ++i) {
            int a, b, c;
            mesh->Triangle(i).Get(a, b, c);
            if (a < 1 || b < 1 || c < 1 || a > nodes || b > nodes || c > nodes) {
                throw std::runtime_error("Invalid IGES triangle index");
            }
            if (face.reversed) {
                std::swap(b, c);
            }
            storage->triangles.push_back({base + static_cast<uint32_t>(a - 1),
                                          base + static_cast<uint32_t>(b - 1),
                                          base + static_cast<uint32_t>(c - 1)});
        }
    }
    return storage;
}
} // namespace

int vtx_iges_load_v1(const char* path, double deflection, VtxIgesMeshV1* output,
                     char* error, size_t error_size) {
    if (!output) {
        return 0;
    }
    *output = {};
    const auto fail = [&](const char* reason) {
        if (error && error_size) {
            std::snprintf(error, error_size, "%s", reason ? reason : "Open CASCADE failed");
        }
        return 0;
    };
    try {
        if (!path || !std::isfinite(deflection) || deflection <= 0.0) {
            return fail("Invalid IGES input or meshing deflection");
        }
        // The data-exchange controller uses process-wide OCCT settings.
        static std::mutex import_mutex;
        const std::lock_guard<std::mutex> lock(import_mutex);
        static const auto quiet = [] {
            Message::DefaultMessenger()->RemovePrinters(STANDARD_TYPE(Message_PrinterOStream));
            return true;
        }();
        (void)quiet;
        const auto start = Clock::now();
        auto read = start;
        TopoDS_Shape shape;
        {
            IGESControl_Reader reader;
            if (reader.ReadFile(path) != IFSelect_RetDone) {
                return fail("Open CASCADE could not read the IGES file");
            }
            read = Clock::now();
            if (reader.TransferRoots() == 0) {
                return fail("IGES file contains no transferable shapes");
            }
            shape = reader.OneShape();
        } // Release the IGES entities and transfer session before allocating the mesh.
        if (shape.IsNull()) {
            return fail("IGES transfer produced no shape");
        }
        const auto transferred = Clock::now();
        BRepMesh_IncrementalMesh mesher(shape, deflection, false, 0.5, true);
        const auto meshed = Clock::now();
        size_t skipped = 0;
        auto storage = extract(shape, skipped);
        const auto finished = Clock::now();
        *output = {storage->positions.front().data(), storage->triangles.front().data(),
                   storage->positions.size(), storage->triangles.size(), skipped,
                   elapsed_ms(start, read), elapsed_ms(read, transferred),
                   elapsed_ms(transferred, meshed), elapsed_ms(meshed, finished), storage.get()};
        storage.release();
        return 1;
    } catch (const Standard_Failure& failure) {
        return fail(failure.GetMessageString());
    } catch (const std::exception& failure) {
        return fail(failure.what());
    } catch (...) {
        return fail("Unknown IGES import failure");
    }
}

void vtx_iges_free_v1(VtxIgesMeshV1* mesh) {
    if (mesh) {
        delete static_cast<Storage*>(mesh->storage);
        *mesh = {};
    }
}
