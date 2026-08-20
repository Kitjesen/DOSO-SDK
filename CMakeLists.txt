cmake_minimum_required(VERSION 3.20)

project(DOSOBrainstemClient VERSION 0.1.0 LANGUAGES CXX)

include(CTest)
include(CMakePackageConfigHelpers)
include(GNUInstallDirs)

option(BRAINSTEM_BUILD_EXAMPLES "Build DOSO Brainstem C++ SDK examples" OFF)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

find_package(Protobuf REQUIRED)
find_package(gRPC CONFIG REQUIRED)
find_package(Threads REQUIRED)

set(BRAINSTEM_API_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/protocol")
set(BRAINSTEM_GENERATED_DIR "${CMAKE_CURRENT_BINARY_DIR}/generated")

if(NOT EXISTS "${BRAINSTEM_API_ROOT}/brainstem_api/cms.proto")
    message(FATAL_ERROR "bundled brainstem_api protocol snapshot is missing")
endif()

set(BRAINSTEM_PROTO_FILES
    brainstem_api/common.proto
    brainstem_api/cms.proto
)

set(BRAINSTEM_GENERATED_SOURCES
    "${BRAINSTEM_GENERATED_DIR}/brainstem_api/common.pb.cc"
    "${BRAINSTEM_GENERATED_DIR}/brainstem_api/common.grpc.pb.cc"
    "${BRAINSTEM_GENERATED_DIR}/brainstem_api/cms.pb.cc"
    "${BRAINSTEM_GENERATED_DIR}/brainstem_api/cms.grpc.pb.cc"
)

set(BRAINSTEM_GENERATED_HEADERS
    "${BRAINSTEM_GENERATED_DIR}/brainstem_api/common.pb.h"
    "${BRAINSTEM_GENERATED_DIR}/brainstem_api/common.grpc.pb.h"
    "${BRAINSTEM_GENERATED_DIR}/brainstem_api/cms.pb.h"
    "${BRAINSTEM_GENERATED_DIR}/brainstem_api/cms.grpc.pb.h"
)

if(TARGET protobuf::protoc)
    set(BRAINSTEM_PROTOC "$<TARGET_FILE:protobuf::protoc>")
else()
    set(BRAINSTEM_PROTOC "${Protobuf_PROTOC_EXECUTABLE}")
endif()

if(TARGET gRPC::grpc_cpp_plugin)
    set(BRAINSTEM_GRPC_CPP_PLUGIN "$<TARGET_FILE:gRPC::grpc_cpp_plugin>")
else()
    find_program(BRAINSTEM_GRPC_CPP_PLUGIN grpc_cpp_plugin REQUIRED)
endif()

set(BRAINSTEM_PROTO_PATH_ARGS "--proto_path=${BRAINSTEM_API_ROOT}")
foreach(PROTOBUF_INCLUDE_DIR IN LISTS Protobuf_INCLUDE_DIRS)
    list(APPEND BRAINSTEM_PROTO_PATH_ARGS "--proto_path=${PROTOBUF_INCLUDE_DIR}")
endforeach()

add_custom_command(
    OUTPUT ${BRAINSTEM_GENERATED_SOURCES} ${BRAINSTEM_GENERATED_HEADERS}
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${BRAINSTEM_GENERATED_DIR}"
    COMMAND "${BRAINSTEM_PROTOC}"
            ${BRAINSTEM_PROTO_PATH_ARGS}
            "--cpp_out=${BRAINSTEM_GENERATED_DIR}"
            "--grpc_out=${BRAINSTEM_GENERATED_DIR}"
            "--plugin=protoc-gen-grpc=${BRAINSTEM_GRPC_CPP_PLUGIN}"
            ${BRAINSTEM_PROTO_FILES}
    WORKING_DIRECTORY "${BRAINSTEM_API_ROOT}"
    DEPENDS
        "${BRAINSTEM_API_ROOT}/brainstem_api/common.proto"
        "${BRAINSTEM_API_ROOT}/brainstem_api/cms.proto"
    COMMENT "Generating Brainstem gRPC client sources"
    VERBATIM
)

add_library(brainstem_client STATIC
    src/client.cpp
    ${BRAINSTEM_GENERATED_SOURCES}
)
add_library(brainstem::client ALIAS brainstem_client)
set_target_properties(brainstem_client PROPERTIES EXPORT_NAME client)

target_include_directories(brainstem_client
    PUBLIC
        "$<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>"
        "$<INSTALL_INTERFACE:${CMAKE_INSTALL_INCLUDEDIR}>"
    PRIVATE
        "${BRAINSTEM_GENERATED_DIR}"
)

target_link_libraries(brainstem_client
    PUBLIC
        gRPC::grpc++
        protobuf::libprotobuf
        Threads::Threads
)

if(BUILD_TESTING)
    add_executable(brainstem_client_test tests/client_test.cpp)
    target_include_directories(brainstem_client_test PRIVATE "${BRAINSTEM_GENERATED_DIR}")
    target_link_libraries(brainstem_client_test PRIVATE brainstem::client)
    add_test(NAME brainstem_client_test COMMAND brainstem_client_test)
endif()

if(BRAINSTEM_BUILD_EXAMPLES)
    add_executable(brainstem_motion_example examples/motion.cpp)
    target_link_libraries(brainstem_motion_example PRIVATE brainstem::client)
endif()

configure_package_config_file(
    cmake/BrainstemClientConfig.cmake.in
    "${CMAKE_CURRENT_BINARY_DIR}/BrainstemClientConfig.cmake"
    INSTALL_DESTINATION "${CMAKE_INSTALL_LIBDIR}/cmake/BrainstemClient"
)
write_basic_package_version_file(
    "${CMAKE_CURRENT_BINARY_DIR}/BrainstemClientConfigVersion.cmake"
    VERSION "${PROJECT_VERSION}"
    COMPATIBILITY SameMajorVersion
)

install(
    TARGETS brainstem_client
    EXPORT BrainstemClientTargets
    ARCHIVE DESTINATION "${CMAKE_INSTALL_LIBDIR}"
    LIBRARY DESTINATION "${CMAKE_INSTALL_LIBDIR}"
    RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}"
    INCLUDES DESTINATION "${CMAKE_INSTALL_INCLUDEDIR}"
)
install(DIRECTORY include/brainstem DESTINATION "${CMAKE_INSTALL_INCLUDEDIR}")
install(
    EXPORT BrainstemClientTargets
    FILE BrainstemClientTargets.cmake
    NAMESPACE brainstem::
    DESTINATION "${CMAKE_INSTALL_LIBDIR}/cmake/BrainstemClient"
)
install(
    FILES
        "${CMAKE_CURRENT_BINARY_DIR}/BrainstemClientConfig.cmake"
        "${CMAKE_CURRENT_BINARY_DIR}/BrainstemClientConfigVersion.cmake"
    DESTINATION "${CMAKE_INSTALL_LIBDIR}/cmake/BrainstemClient"
)
