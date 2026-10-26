// SPDX-License-Identifier: GPL-3.0-or-later
// Original synthetic corpus and bounded memory callbacks. No upstream PoC data.
#include <charls/charls.h>
#include <openjpeg.h>
#include <algorithm>
#include <cstdint>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <stdexcept>
#include <string>
#include <vector>

using Bytes = std::vector<uint8_t>;
static size_t cases = 0;
static void require(bool value, const char* message) {
    ++cases;
    if (!value) throw std::runtime_error(message);
}
static std::vector<size_t> markers(const Bytes& source, uint8_t code) {
    std::vector<size_t> result;
    for (size_t i = 0; i + 1 < source.size(); ++i)
        if (source[i] == 0xff && source[i + 1] == code) result.push_back(i);
    return result;
}
static bool jls(const Bytes& source) {
    try {
        charls::jpegls_decoder decoder(source.data(), source.size());
        if (decoder.destination_size() > 16 * 1024 * 1024) return false;
        return !decoder.decode<Bytes>().empty();
    } catch (const charls::jpegls_error&) { return false; }
}
struct Input { const Bytes& bytes; size_t position = 0; };
static OPJ_SIZE_T read_memory(void* destination, OPJ_SIZE_T requested, void* user) {
    auto& input = *static_cast<Input*>(user);
    if (input.position == input.bytes.size()) return static_cast<OPJ_SIZE_T>(-1);
    const size_t count = std::min<size_t>(requested, input.bytes.size() - input.position);
    std::memcpy(destination, input.bytes.data() + input.position, count);
    input.position += count;
    return count;
}
static OPJ_OFF_T skip_memory(OPJ_OFF_T count, void* user) {
    auto& input = *static_cast<Input*>(user);
    if (count < 0) {
        // Avoid negating INT64_MIN from an invalid native seek request.
        const uint64_t backwards = static_cast<uint64_t>(-(count + 1)) + 1;
        if (backwards > input.position) return -1;
        input.position -= static_cast<size_t>(backwards);
    } else {
        if (static_cast<uint64_t>(count) > input.bytes.size() - input.position) return -1;
        input.position += static_cast<size_t>(count);
    }
    return count;
}
static OPJ_BOOL seek_memory(OPJ_OFF_T position, void* user) {
    auto& input = *static_cast<Input*>(user);
    if (position < 0 || static_cast<uint64_t>(position) > input.bytes.size()) return OPJ_FALSE;
    input.position = static_cast<size_t>(position);
    return OPJ_TRUE;
}
static bool j2k(const Bytes& source, bool strict = true) {
    Input input{source};
    opj_stream_t* stream = opj_stream_create(4096, OPJ_TRUE);
    opj_codec_t* codec = opj_create_decompress(OPJ_CODEC_J2K);
    if (!stream || !codec) throw std::runtime_error("Native codec allocation failed");
    opj_stream_set_user_data(stream, &input, nullptr);
    opj_stream_set_user_data_length(stream, source.size());
    opj_stream_set_read_function(stream, read_memory);
    opj_stream_set_skip_function(stream, skip_memory);
    opj_stream_set_seek_function(stream, seek_memory);
    opj_dparameters_t parameters;
    opj_set_default_decoder_parameters(&parameters);
    opj_image_t* image = nullptr;
    bool result = opj_setup_decoder(codec, &parameters) && opj_decoder_set_strict_mode(codec, strict)
        && opj_read_header(stream, codec, &image);
    if (result) {
        result = image && image->x1 >= image->x0 && image->y1 >= image->y0
            && image->x1 - image->x0 <= 1024 && image->y1 - image->y0 <= 1024
            && opj_decode(codec, stream, image) && opj_end_decompress(codec, stream);
    }
    if (image) opj_image_destroy(image);
    opj_destroy_codec(codec);
    opj_stream_destroy(stream);
    return result;
}
int main(int argc, char** argv) {
    try {
        if (argc != 2) throw std::runtime_error("Pass the repository's synthetic fixture directory");
        require(std::string(charls_get_version_string()) == "2.4.5", "Unexpected linked CharLS version");
        require(std::string(opj_version()) == "2.5.4", "Unexpected linked OpenJPEG baseline version");
        Bytes pixels(768);
        for (size_t i = 0; i < pixels.size(); ++i) pixels[i] = static_cast<uint8_t>(i % 251);
        const auto planar = charls::jpegls_encoder::encode(pixels, {64,4,8,3});
        const auto scans = markers(planar, 0xda);
        require(scans.size() == 3, "Expected three synthetic planar scans");
        require(jls(planar), "Valid JPEG-LS failed");
        for (auto mode : {charls::interleave_mode::line, charls::interleave_mode::sample}) {
            const auto packed = charls::jpegls_encoder::encode(pixels, {64,4,8,3}, mode);
            const auto starts = markers(packed, 0xda);
            require(starts.size() == 1 && jls(packed), "Valid packed JPEG-LS failed");
            Bytes mixed(planar.begin(), planar.begin() + scans[2]);
            mixed.insert(mixed.end(), packed.begin() + starts[0], packed.end());
            require(!jls(mixed), "Mixed scan layout was accepted");
        }
        for (size_t cut : {size_t{8}, planar.size()/2, planar.size()-1})
            require(!jls(Bytes(planar.begin(), planar.begin()+cut)), "Truncated JPEG-LS was accepted");
        for (const char* name : {"synthetic-u16.j2k", "synthetic-rgb.j2k", "synthetic-rgb16.j2k", "synthetic-u16-lossy.j2k"}) {
            std::ifstream file(std::filesystem::path(argv[1]) / name, std::ios::binary);
            if (!file) throw std::runtime_error("Missing synthetic JPEG 2000 fixture");
            Bytes source{std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()};
            require(j2k(source), "Valid JPEG 2000 failed");
            for (size_t cut : {size_t{8}, size_t{42}, source.size()/2, source.size()-2, source.size()-1})
                require(!j2k(Bytes(source.begin(), source.begin()+cut)), "Truncated JPEG 2000 was accepted");
            const auto cod = markers(source, 0x52).at(0), sot = markers(source, 0x90).at(0);
            auto changed = source;
            changed[cod+9] = 255;
            require(!j2k(changed), "Invalid decomposition count was accepted");
            changed = source;
            changed[sot+10] = 255; changed[sot+11] = 1;
            require(!j2k(changed), "Invalid tile part was accepted");
            Bytes empty(source.begin(), source.begin()+sot+12);
            empty[cod+6] = empty[cod+7] = 255;
            empty[sot+6] = empty[sot+7] = empty[sot+8] = 0; empty[sot+9] = 14;
            empty.insert(empty.end(), {0xff,0x93,0xff,0xd9});
            require(!j2k(empty), "Strict empty tile payload was accepted");
            (void)j2k(empty, false); ++cases; // Exhausted-header loop must terminate.
        }
        std::cout << "Native ASan/UBSan checks completed: " << cases << " cases\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "Native regression failed: " << error.what() << '\n';
        return 1;
    }
}
