cmake_minimum_required(VERSION 3.10)
enable_testing()
project(neomaster)

set(CMAKE_BUILD_TYPE Debug)

###
# Protobufstuf
###

find_package(SDL2 REQUIRED)
find_package(LibUSB REQUIRED)
find_package(Protobuf REQUIRED)

include_directories(${Protobuf_INCLUDE_DIRS})
include_directories(${CMAKE_CURRENT_BINARY_DIR})

protobuf_generate_cpp(PROTO_SRCS PROTO_HDRS protocols/beosource.proto)
protobuf_generate_python(PROTO_PY foo.proto)

set(CMAKE_CXX_STANDARD 14)
set(CMAKE_CXX_STANDARD_REQUIRED True)


###
# Boost
###

set(Boost_ADDITIONAL_VERSIONS "1.67" "1.67.0")
find_package(Boost 1.67 REQUIRED COMPONENTS log iostreams system thread)
add_definitions(-DBOOST_LOG_DYN_LINK)

### 
# GUnit stuff 
###

configure_file(CMakeLists.txt.in ${CMAKE_BINARY_DIR}/GUnit-download/CMakeLists.txt)
execute_process(COMMAND ${CMAKE_COMMAND} -G "${CMAKE_GENERATOR}" .
  RESULT_VARIABLE result
  WORKING_DIRECTORY ${CMAKE_BINARY_DIR}/GUnit-download )
if(result)
  message(FATAL_ERROR "CMake step for gunit failed: ${result}")
endif()
execute_process(COMMAND ${CMAKE_COMMAND} --build .
  RESULT_VARIABLE result
  WORKING_DIRECTORY ${CMAKE_BINARY_DIR}/GUnit-download )
if(result)
  message(FATAL_ERROR "Build step for gunit failed: ${result}")
endif()


## Add googletest directly to our build. This defines
## the gtest and gtest_main targets.
add_subdirectory(${CMAKE_BINARY_DIR}/gunit-src
                 ${CMAKE_BINARY_DIR}/gunit-build
                 EXCLUDE_FROM_ALL)

###
# End gunit
###

list(APPEND SRC_FILES 
    sources/spotify/event_handler.cpp
    sources/spotify/spotify_source.cpp
    sources/spotify/transport.cpp
)

list(APPEND TESTING_SRC_FILES 
    sources/spotify/transport_test.cpp
    sources/spotify/event_handler_test.cpp
    )


add_executable(neomaster_test ${TESTING_SRC_FILES} ${SRC_FILES} ${PROTO_SRCS} ${PROTO_HDRS})
add_executable(neomaster core.cpp ${SRC_FILES} ${PROTO_SRCS} ${PROTO_HDRS})

add_test(NAME UnitTests COMMAND neomaster_test)

target_include_directories(neomaster PUBLIC libpc2)
target_include_directories(neomaster PUBLIC .)
target_include_directories(neomaster PUBLIC /usr/include/dbus-c++-1/)

target_include_directories(neomaster_test PUBLIC ${GUnit_SOURCE_DIR}/include)
target_include_directories(neomaster_test PUBLIC ${gtest_SOURCE_DIR}/include)
target_include_directories(neomaster_test PUBLIC ${gmock_SOURCE_DIR}/include)
target_include_directories(neomaster_test PUBLIC libpc2)
target_include_directories(neomaster_test PUBLIC .)
target_include_directories(neomaster_test PUBLIC /usr/include/dbus-c++-1/)

target_link_libraries(neomaster usb-1.0)
target_link_libraries(neomaster Boost::log)
target_link_libraries(neomaster SDL2)
target_link_libraries(neomaster SDL2_ttf)
target_link_libraries(neomaster SDL2_image)
target_link_libraries(neomaster ${CMAKE_SOURCE_DIR}/libpc2/libpc2.a)
target_link_libraries(neomaster /usr/lib/x86_64-linux-gnu/libdbus-c++-1.so)
target_link_libraries(neomaster ${Protobuf_LIBRARIES})

target_link_libraries(neomaster_test gtest_main)
target_link_libraries(neomaster_test gmock_main)
target_link_libraries(neomaster_test usb-1.0)
target_link_libraries(neomaster_test Boost::log)
target_link_libraries(neomaster_test SDL2)
target_link_libraries(neomaster_test SDL2_ttf)
target_link_libraries(neomaster_test SDL2_image)
target_link_libraries(neomaster_test ${CMAKE_SOURCE_DIR}/libpc2/libpc2.a)
target_link_libraries(neomaster_test /usr/lib/x86_64-linux-gnu/libdbus-c++-1.so)
target_link_libraries(neomaster_test ${Protobuf_LIBRARIES})
