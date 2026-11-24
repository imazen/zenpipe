#include <emscripten/emscripten.h>
#include <jxl/cms.h>
#include <jxl/color_encoding.h>
#include <jxl/decode.h>
#include <jxl/decode_cxx.h>
#include <jxl/types.h>

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

namespace {

constexpr uint64_t kMaxPixels = 64000000ULL;
constexpr uint32_t kMaxSide = 16384U;
constexpr uint32_t kMaxFrames = 256U;

std::string g_json;
std::vector<uint8_t> g_pixels;
std::string g_version;

std::string json_escape(const std::string& s) {
  std::string out;
  out.reserve(s.size() + 8);
  for (unsigned char c : s) {
    switch (c) {
      case '"': out += "\\\""; break;
      case '\\': out += "\\\\"; break;
      case '\b': out += "\\b"; break;
      case '\f': out += "\\f"; break;
      case '\n': out += "\\n"; break;
      case '\r': out += "\\r"; break;
      case '\t': out += "\\t"; break;
      default:
        if (c < 0x20) {
          char buf[8];
          std::snprintf(buf, sizeof(buf), "\\u%04x", static_cast<unsigned>(c));
          out += buf;
        } else {
          out.push_back(static_cast<char>(c));
        }
        break;
    }
  }
  return out;
}

const char* status_name(JxlDecoderStatus st) {
  switch (st) {
    case JXL_DEC_SUCCESS: return "JXL_DEC_SUCCESS";
    case JXL_DEC_ERROR: return "JXL_DEC_ERROR";
    case JXL_DEC_NEED_MORE_INPUT: return "JXL_DEC_NEED_MORE_INPUT";
    case JXL_DEC_NEED_PREVIEW_OUT_BUFFER: return "JXL_DEC_NEED_PREVIEW_OUT_BUFFER";
    case JXL_DEC_NEED_IMAGE_OUT_BUFFER: return "JXL_DEC_NEED_IMAGE_OUT_BUFFER";
    case JXL_DEC_BASIC_INFO: return "JXL_DEC_BASIC_INFO";
    case JXL_DEC_COLOR_ENCODING: return "JXL_DEC_COLOR_ENCODING";
    case JXL_DEC_PREVIEW_IMAGE: return "JXL_DEC_PREVIEW_IMAGE";
    case JXL_DEC_FRAME: return "JXL_DEC_FRAME";
    case JXL_DEC_FULL_IMAGE: return "JXL_DEC_FULL_IMAGE";
    case JXL_DEC_BOX: return "JXL_DEC_BOX";
    case JXL_DEC_BOX_NEED_MORE_OUTPUT: return "JXL_DEC_BOX_NEED_MORE_OUTPUT";
    default: return "JXL_DEC_UNKNOWN";
  }
}

std::string format_version() {
  uint32_t v = JxlDecoderVersion();
  uint32_t major = v / 1000000;
  uint32_t minor = (v / 1000) % 1000;
  uint32_t patch = v % 1000;
  char buf[96];
  std::snprintf(buf, sizeof(buf), "libjxl %u.%u.%u (lcms2 wasm)", major, minor, patch);
  return std::string(buf);
}

void build_json(
    bool ok,
    const std::string& error,
    uint32_t width,
    uint32_t height,
    uint32_t frame_count,
    const std::vector<uint32_t>& durations_ms,
    bool has_loop_count,
    uint32_t loop_count,
    bool has_alpha,
    bool has_icc,
    bool is_cmyk,
    bool uses_original_profile,
    uint32_t extra_channels,
    uint32_t orientation,
    const std::string& cms_used,
    const std::vector<std::string>& warnings) {
  std::string out = "{";
  out += "\"decoder\":\"libjxl\",";
  out += "\"version\":\"" + json_escape(format_version()) + "\",";
  out += "\"ok\":" + std::string(ok ? "true" : "false") + ",";
  if (error.empty()) {
    out += "\"error\":null,";
  } else {
    out += "\"error\":\"" + json_escape(error) + "\",";
  }
  out += "\"width\":" + std::to_string(width) + ",";
  out += "\"height\":" + std::to_string(height) + ",";
  out += "\"frameCount\":" + std::to_string(frame_count) + ",";
  out += "\"frameDurationsMs\":[";
  for (size_t i = 0; i < durations_ms.size(); ++i) {
    if (i) out += ",";
    out += std::to_string(durations_ms[i]);
  }
  out += "],";
  if (has_loop_count) {
    out += "\"loopCount\":" + std::to_string(loop_count) + ",";
  } else {
    out += "\"loopCount\":null,";
  }
  out += "\"hasAlpha\":" + std::string(has_alpha ? "true" : "false") + ",";
  out += "\"hasIcc\":" + std::string(has_icc ? "true" : "false") + ",";
  out += "\"isCmyk\":" + std::string(is_cmyk ? "true" : "false") + ",";
  out += "\"usesOriginalProfile\":" + std::string(uses_original_profile ? "true" : "false") + ",";
  out += "\"extraChannels\":" + std::to_string(extra_channels) + ",";
  out += "\"orientation\":" + std::to_string(orientation) + ",";
  out += "\"cmsUsed\":\"" + json_escape(cms_used) + "\",";
  out += "\"level5Accepted\":null,";
  out += "\"warnings\":[";
  for (size_t i = 0; i < warnings.size(); ++i) {
    if (i) out += ",";
    out += "\"" + json_escape(warnings[i]) + "\"";
  }
  out += "]}";
  g_json = std::move(out);
}

}  // namespace

extern "C" {

EMSCRIPTEN_KEEPALIVE
const char* libjxl_version_str() {
  if (g_version.empty()) {
    g_version = format_version();
  }
  return g_version.c_str();
}

EMSCRIPTEN_KEEPALIVE
void libjxl_free_result() {
  g_pixels.clear();
  g_pixels.shrink_to_fit();
  g_json.clear();
}

EMSCRIPTEN_KEEPALIVE
const char* libjxl_result_json() {
  return g_json.c_str();
}

EMSCRIPTEN_KEEPALIVE
const uint8_t* libjxl_result_pixels() {
  return g_pixels.empty() ? nullptr : g_pixels.data();
}

EMSCRIPTEN_KEEPALIVE
size_t libjxl_result_pixels_len() {
  return g_pixels.size();
}

EMSCRIPTEN_KEEPALIVE
int libjxl_decode_rgba(const uint8_t* data, size_t len) {
  g_pixels.clear();
  g_json.clear();

  uint32_t width = 0;
  uint32_t height = 0;
  uint32_t frame_count = 0;
  std::vector<uint32_t> durations_ms;
  bool has_loop_count = false;
  uint32_t loop_count = 0;
  bool has_alpha = false;
  bool has_icc = false;
  bool is_cmyk = false;
  bool uses_original_profile = false;
  uint32_t extra_channels = 0;
  uint32_t orientation = 1;
  std::string cms_used = "lcms2";
  std::vector<std::string> warnings;

  if (!data || len == 0) {
    build_json(false, "Empty input buffer.", 0, 0, 0, durations_ms, false, 0,
               false, false, false, false, 0, 1, cms_used, warnings);
    return 1;
  }

  auto dec = JxlDecoderMake(nullptr);
  if (!dec) {
    build_json(false, "JxlDecoderCreate failed.", 0, 0, 0, durations_ms, false, 0,
               false, false, false, false, 0, 1, cms_used, warnings);
    return 1;
  }

  const JxlCmsInterface* default_cms = JxlGetDefaultCms();
  if (default_cms) {
    if (JxlDecoderSetCms(dec.get(), *default_cms) != JXL_DEC_SUCCESS) {
      cms_used = "none";
      warnings.push_back("JxlDecoderSetCms failed; decoding without CMS.");
    }
  } else {
    cms_used = "none";
  }

  JxlDecoderSetCoalescing(dec.get(), JXL_TRUE);
  JxlDecoderSetKeepOrientation(dec.get(), JXL_FALSE);
  JxlDecoderSetUnpremultiplyAlpha(dec.get(), JXL_TRUE);

  if (JxlDecoderSubscribeEvents(
          dec.get(),
          JXL_DEC_BASIC_INFO | JXL_DEC_COLOR_ENCODING | JXL_DEC_FRAME | JXL_DEC_FULL_IMAGE) !=
      JXL_DEC_SUCCESS) {
    build_json(false, "JxlDecoderSubscribeEvents failed.", 0, 0, 0, durations_ms, false, 0,
               false, false, false, false, 0, 1, cms_used, warnings);
    return 1;
  }

  if (JxlDecoderSetInput(dec.get(), data, len) != JXL_DEC_SUCCESS) {
    build_json(false, "JxlDecoderSetInput failed.", 0, 0, 0, durations_ms, false, 0,
               false, false, false, false, 0, 1, cms_used, warnings);
    return 1;
  }
  JxlDecoderCloseInput(dec.get());

  JxlBasicInfo info{};
  bool have_info = false;
  JxlPixelFormat format{4, JXL_TYPE_UINT8, JXL_NATIVE_ENDIAN, 0};
  std::vector<uint8_t> scratch;
  bool srgb_output_set = false;

  for (;;) {
    JxlDecoderStatus status = JxlDecoderProcessInput(dec.get());
    if (status == JXL_DEC_ERROR) {
      build_json(false, "libjxl returned JXL_DEC_ERROR while decoding.",
                 width, height, frame_count, durations_ms, has_loop_count, loop_count,
                 has_alpha, has_icc, is_cmyk, uses_original_profile, extra_channels,
                 orientation, cms_used, warnings);
      g_pixels.clear();
      return 1;
    }
    if (status == JXL_DEC_NEED_MORE_INPUT) {
      build_json(false, "Truncated JPEG XL stream (JXL_DEC_NEED_MORE_INPUT after CloseInput).",
                 width, height, frame_count, durations_ms, has_loop_count, loop_count,
                 has_alpha, has_icc, is_cmyk, uses_original_profile, extra_channels,
                 orientation, cms_used, warnings);
      g_pixels.clear();
      return 1;
    }
    if (status == JXL_DEC_BASIC_INFO) {
      if (JxlDecoderGetBasicInfo(dec.get(), &info) != JXL_DEC_SUCCESS) {
        build_json(false, "JxlDecoderGetBasicInfo failed.", 0, 0, 0, durations_ms, false, 0,
                   false, false, false, false, 0, 1, cms_used, warnings);
        return 1;
      }
      have_info = true;
      width = info.xsize;
      height = info.ysize;
      orientation = static_cast<uint32_t>(info.orientation);
      extra_channels = info.num_extra_channels;
      has_alpha = info.alpha_bits > 0;
      uses_original_profile = info.uses_original_profile == JXL_TRUE;
      if (info.have_animation) {
        has_loop_count = true;
        loop_count = info.animation.num_loops;
      }
      for (uint32_t i = 0; i < info.num_extra_channels; ++i) {
        JxlExtraChannelInfo eci{};
        if (JxlDecoderGetExtraChannelInfo(dec.get(), i, &eci) == JXL_DEC_SUCCESS) {
          if (eci.type == JXL_CHANNEL_BLACK) {
            is_cmyk = true;
          } else if (eci.type == JXL_CHANNEL_ALPHA) {
            has_alpha = true;
          }
        }
      }
      if (width == 0 || height == 0 || width > kMaxSide || height > kMaxSide ||
          static_cast<uint64_t>(width) * static_cast<uint64_t>(height) > kMaxPixels) {
        build_json(false, "Decoded dimensions exceed 16384 px / 64 MP safety limit.",
                   width, height, 0, durations_ms, has_loop_count, loop_count,
                   has_alpha, has_icc, is_cmyk, uses_original_profile, extra_channels,
                   orientation, cms_used, warnings);
        return 1;
      }
    } else if (status == JXL_DEC_COLOR_ENCODING) {
      size_t icc_size = 0;
      if (JxlDecoderGetICCProfileSize(dec.get(), JXL_COLOR_PROFILE_TARGET_ORIGINAL, &icc_size) ==
              JXL_DEC_SUCCESS &&
          icc_size > 0) {
        has_icc = true;
      }
      // Request sRGB output so embedded ICC / CMYK profiles convert via lcms2 to sRGB RGBA8.
      JxlColorEncoding srgb{};
      srgb.color_space = JXL_COLOR_SPACE_RGB;
      srgb.white_point = JXL_WHITE_POINT_D65;
      srgb.primaries = JXL_PRIMARIES_SRGB;
      srgb.transfer_function = JXL_TRANSFER_FUNCTION_SRGB;
      srgb.rendering_intent = JXL_RENDERING_INTENT_RELATIVE;
      if (JxlDecoderSetOutputColorProfile(dec.get(), &srgb, nullptr, 0) == JXL_DEC_SUCCESS) {
        srgb_output_set = true;
      } else {
        warnings.push_back("JxlDecoderSetOutputColorProfile(sRGB) rejected; using decoder default output profile.");
      }
    } else if (status == JXL_DEC_FRAME) {
      if (frame_count >= kMaxFrames) {
        warnings.push_back("Stopped decoding after 256 frames (frame cap).");
        break;
      }
      JxlFrameHeader fh{};
      if (JxlDecoderGetFrameHeader(dec.get(), &fh) == JXL_DEC_SUCCESS && have_info &&
          info.have_animation && info.animation.tps_numerator > 0) {
        double ms = (static_cast<double>(fh.duration) *
                     static_cast<double>(info.animation.tps_denominator) * 1000.0) /
                    static_cast<double>(info.animation.tps_numerator);
        durations_ms.push_back(static_cast<uint32_t>(std::llround(ms)));
      }
    } else if (status == JXL_DEC_NEED_IMAGE_OUT_BUFFER) {
      size_t buffer_size = 0;
      if (JxlDecoderImageOutBufferSize(dec.get(), &format, &buffer_size) != JXL_DEC_SUCCESS) {
        build_json(false, "JxlDecoderImageOutBufferSize failed.",
                   width, height, frame_count, durations_ms, has_loop_count, loop_count,
                   has_alpha, has_icc, is_cmyk, uses_original_profile, extra_channels,
                   orientation, cms_used, warnings);
        g_pixels.clear();
        return 1;
      }
      std::vector<uint8_t>& target = (frame_count == 0) ? g_pixels : scratch;
      target.resize(buffer_size);
      if (JxlDecoderSetImageOutBuffer(dec.get(), &format, target.data(), target.size()) !=
          JXL_DEC_SUCCESS) {
        build_json(false, "JxlDecoderSetImageOutBuffer failed.",
                   width, height, frame_count, durations_ms, has_loop_count, loop_count,
                   has_alpha, has_icc, is_cmyk, uses_original_profile, extra_channels,
                   orientation, cms_used, warnings);
        g_pixels.clear();
        return 1;
      }
    } else if (status == JXL_DEC_FULL_IMAGE) {
      ++frame_count;
    } else if (status == JXL_DEC_SUCCESS) {
      break;
    } else {
      std::string msg = std::string("Unexpected libjxl status: ") + status_name(status);
      build_json(false, msg, width, height, frame_count, durations_ms, has_loop_count, loop_count,
                 has_alpha, has_icc, is_cmyk, uses_original_profile, extra_channels,
                 orientation, cms_used, warnings);
      g_pixels.clear();
      return 1;
    }
  }

  if (!srgb_output_set && has_icc) {
    cms_used = "default";
  }

  if (frame_count == 0 || g_pixels.size() != static_cast<size_t>(width) * static_cast<size_t>(height) * 4U) {
    build_json(false, "libjxl produced no valid RGBA8 frame.",
               width, height, frame_count, durations_ms, has_loop_count, loop_count,
               has_alpha, has_icc, is_cmyk, uses_original_profile, extra_channels,
               orientation, cms_used, warnings);
    g_pixels.clear();
    return 1;
  }

  build_json(true, "", width, height, frame_count, durations_ms, has_loop_count, loop_count,
             has_alpha, has_icc, is_cmyk, uses_original_profile, extra_channels,
             orientation, cms_used, warnings);
  return 0;
}

}  // extern "C"
