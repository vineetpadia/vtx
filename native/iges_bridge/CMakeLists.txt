cmake_minimum_required(VERSION 3.20)
project(vtx_iges LANGUAGES CXX)

find_package(OpenCASCADE CONFIG REQUIRED COMPONENTS FoundationClasses ModelingData ModelingAlgorithms)
find_library(OCCT_IGES NAMES TKDEIGES TKIGES PATHS "${OpenCASCADE_LIBRARY_DIR}" NO_DEFAULT_PATH REQUIRED)

add_library(vtx_iges SHARED iges_bridge.cpp)
target_compile_features(vtx_iges PRIVATE cxx_std_17)
target_include_directories(vtx_iges SYSTEM PRIVATE "${OpenCASCADE_INCLUDE_DIR}")
target_link_libraries(vtx_iges PRIVATE "${OCCT_IGES}")
foreach(toolkit TKMesh TKBRep TKTopAlgo TKXSBase TKMath TKernel)
    find_library(OCCT_${toolkit} NAMES ${toolkit} PATHS "${OpenCASCADE_LIBRARY_DIR}" NO_DEFAULT_PATH REQUIRED)
    target_link_libraries(vtx_iges PRIVATE "${OCCT_${toolkit}}")
endforeach()
set_target_properties(vtx_iges PROPERTIES
    CXX_VISIBILITY_PRESET hidden VISIBILITY_INLINES_HIDDEN ON INSTALL_RPATH_USE_LINK_PATH ON)

if(WIN32)
    target_compile_definitions(vtx_iges PRIVATE _USE_MATH_DEFINES)
    set_target_properties(vtx_iges PROPERTIES PREFIX "")
elseif(APPLE)
    set_target_properties(vtx_iges PROPERTIES INSTALL_RPATH "@loader_path")
else()
    set_target_properties(vtx_iges PROPERTIES INSTALL_RPATH "$ORIGIN")
endif()

install(TARGETS vtx_iges RUNTIME DESTINATION . LIBRARY DESTINATION .)
