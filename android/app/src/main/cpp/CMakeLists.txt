cmake_minimum_required(VERSION 3.22.1)
project(rcwlp_evdev C)

add_library(rcwlp_evdev SHARED evdev.c)
target_compile_options(rcwlp_evdev PRIVATE -Wall -Wextra -Werror -Wno-unused-parameter)
