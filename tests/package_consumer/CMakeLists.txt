cmake_minimum_required(VERSION 3.20)
project(BrainstemClientConsumer LANGUAGES CXX)

find_package(BrainstemClient CONFIG REQUIRED)

add_executable(brainstem_client_consumer main.cpp)
target_link_libraries(brainstem_client_consumer PRIVATE brainstem::client)
