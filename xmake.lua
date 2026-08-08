add_rules("mode.debug", "mode.release")

-- Define external dependencies from xrepo
add_requires("mimalloc")
add_requires("simdjson")
add_requires("spdlog")
add_requires("fmt")
add_requires("tbb")
add_requires("libcurl")

target("syspilot")
    set_kind("binary")
    set_languages("c++17")

    -- Add source files
    add_files("src/*.cpp", "src/ui/*.cpp")

    -- Include directories
    add_includedirs("src", "src/vendor")

    -- Link dependencies from xrepo
    add_packages("mimalloc", "simdjson", "spdlog", "fmt", "tbb", "libcurl")

    -- Add system links
    add_syslinks("pthread")

    -- Compiler warning flags
    set_warnings("all", "extra")
    add_cxflags("-Wno-unused-parameter")

    -- Optimization and release flags mirroring the build.sh performance setup
    if is_mode("release") then
        set_symbols("hidden")
        set_optimize("fastest")
        add_cxflags("-Ofast", "-flto", "-march=native", "-fomit-frame-pointer", "-funroll-loops", "-fno-plt", "-ffast-math")
        add_ldflags("-flto", "-march=native")
        add_defines("NDEBUG")
    end
