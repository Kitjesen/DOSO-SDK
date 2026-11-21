cmake_minimum_required(VERSION 3.16)
project(doso_cpp_sdk LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

add_library(doso_cpp_sdk INTERFACE)
target_include_directories(doso_cpp_sdk INTERFACE
    ${CMAKE_CURRENT_SOURCE_DIR}/include
    ${CMAKE_CURRENT_SOURCE_DIR}/../ffi/include
)
if (UNIX AND NOT APPLE)
    target_link_libraries(doso_cpp_sdk INTERFACE dl)
endif()

add_executable(doso_cpp_smoke tests/smoke_test.cpp)
target_link_libraries(doso_cpp_smoke PRIVATE doso_cpp_sdk)

add_executable(doso_cpp_custom_rl_example examples/custom_rl_policy.cpp)
target_link_libraries(doso_cpp_custom_rl_example PRIVATE doso_cpp_sdk)
