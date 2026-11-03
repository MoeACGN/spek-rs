/* Static FFmpeg decoder. Each frame is converted to planar f32 and handed
   to the callback, so the caller can show progress without a second copy. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/avutil.h>
#include <libavutil/channel_layout.h>
#include <libavutil/error.h>
#include <libavutil/rational.h>
#include <libavutil/samplefmt.h>

typedef struct SpekInfo {
    int sample_rate;
    int bits_per_sample;
    int stream_count;
    int stream_index;
    int bit_rate;
    int channels;
    int64_t total_samples;
    char codec[160];
} SpekInfo;

typedef int (*SpekFrameFn)(
    void *user,
    const float *const *planes,
    int channels,
    int nb_samples,
    const SpekInfo *info);

static void set_err(char *err, int err_len, const char *msg)
{
    if (err && err_len > 0) {
        snprintf(err, (size_t)err_len, "%s", msg);
    }
}

static void set_av_err(char *err, int err_len, const char *prefix, int averr)
{
    char buf[160];
    av_strerror(averr, buf, sizeof(buf));
    if (err && err_len > 0) {
        snprintf(err, (size_t)err_len, "%s：%s", prefix, buf);
    }
}

static int supported_fmt(enum AVSampleFormat fmt)
{
    switch (fmt) {
    case AV_SAMPLE_FMT_U8:
    case AV_SAMPLE_FMT_U8P:
    case AV_SAMPLE_FMT_S16:
    case AV_SAMPLE_FMT_S16P:
    case AV_SAMPLE_FMT_S32:
    case AV_SAMPLE_FMT_S32P:
    case AV_SAMPLE_FMT_S64:
    case AV_SAMPLE_FMT_S64P:
    case AV_SAMPLE_FMT_FLT:
    case AV_SAMPLE_FMT_FLTP:
    case AV_SAMPLE_FMT_DBL:
    case AV_SAMPLE_FMT_DBLP:
        return 1;
    default:
        return 0;
    }
}

static int64_t estimate_total(AVFormatContext *fmt, AVStream *stream, int sample_rate)
{
    double seconds = 0.0;
    if (sample_rate <= 0) {
        return 0;
    }
    if (stream && stream->duration > 0 && stream->time_base.den > 0) {
        seconds = stream->duration * av_q2d(stream->time_base);
    }
    if (seconds <= 0.0 && fmt && fmt->duration > 0) {
        seconds = (double)fmt->duration / (double)AV_TIME_BASE;
    }
    if (seconds <= 0.0) {
        return 0;
    }
    return (int64_t)(seconds * (double)sample_rate + 0.5);
}

static void copy_codec_name(char *dst, size_t dst_len, const AVCodec *codec)
{
    const char *name = "FFmpeg";
    if (codec && codec->long_name && codec->long_name[0]) {
        name = codec->long_name;
    } else if (codec && codec->name) {
        name = codec->name;
    }
    snprintf(dst, dst_len, "%s", name);
}

static int emit_frame(const AVFrame *frame, SpekFrameFn fn, void *user, SpekInfo *info)
{
    int channels = frame->ch_layout.nb_channels;
    int n = frame->nb_samples;
    int planar;
    int c;
    int i;
    float **planes;
    int stop;
    if (!fn || n <= 0 || channels <= 0) {
        return 0;
    }
    if (!supported_fmt(frame->format)) {
        return -2;
    }
    planes = (float **)calloc((size_t)channels, sizeof(float *));
    if (!planes) {
        return -1;
    }
    for (c = 0; c < channels; c++) {
        planes[c] = (float *)malloc((size_t)n * sizeof(float));
        if (!planes[c]) {
            stop = -1;
            goto done;
        }
    }
    planar = av_sample_fmt_is_planar(frame->format);
    for (c = 0; c < channels; c++) {
        const uint8_t *base = planar ? frame->data[c] : frame->data[0];
        float *dst = planes[c];
        if (frame->format == AV_SAMPLE_FMT_FLTP) {
            memcpy(dst, base, (size_t)n * sizeof(float));
            continue;
        }
        for (i = 0; i < n; i++) {
            int index = planar ? i : i * channels + c;
            float value = 0.0f;
            switch (frame->format) {
            case AV_SAMPLE_FMT_U8:
            case AV_SAMPLE_FMT_U8P:
                value = (((const uint8_t *)base)[index] - 128) / 128.0f;
                break;
            case AV_SAMPLE_FMT_S16:
            case AV_SAMPLE_FMT_S16P:
                value = ((const int16_t *)base)[index] / 32767.0f;
                break;
            case AV_SAMPLE_FMT_S32:
            case AV_SAMPLE_FMT_S32P:
                value = ((const int32_t *)base)[index] / 2147483647.0f;
                break;
            case AV_SAMPLE_FMT_S64:
            case AV_SAMPLE_FMT_S64P:
                value = (float)(((const int64_t *)base)[index] / (double)INT64_MAX);
                break;
            case AV_SAMPLE_FMT_FLT:
                value = ((const float *)base)[index];
                break;
            case AV_SAMPLE_FMT_DBL:
            case AV_SAMPLE_FMT_DBLP:
                value = (float)((const double *)base)[index];
                break;
            default:
                break;
            }
            dst[i] = value;
        }
    }
    info->channels = channels;
    if (info->sample_rate <= 0 && frame->sample_rate > 0) {
        info->sample_rate = frame->sample_rate;
    }
    stop = fn(user, (const float *const *)planes, channels, n, info);
done:
    for (c = 0; c < channels; c++) {
        free(planes[c]);
    }
    free(planes);
    return stop;
}

int spek_ffmpeg_decode(
    const char *path,
    int stream_ordinal,
    int threads,
    SpekFrameFn fn,
    void *user,
    char *err,
    int err_len)
{
    AVFormatContext *fmt = NULL;
    AVCodecContext *ctx = NULL;
    const AVCodec *codec = NULL;
    AVPacket *packet = NULL;
    AVFrame *frame = NULL;
    SpekInfo info;
    int audio_seen = 0;
    int audio_abs = -1;
    unsigned i;
    int ret;
    int emitted = 0;
    AVCodecParameters *par = NULL;
    AVStream *stream = NULL;

    if (err && err_len > 0) {
        err[0] = 0;
    }
    memset(&info, 0, sizeof(info));
    if (!path || !path[0] || !fn) {
        set_err(err, err_len, "沒有檔案路徑");
        return -1;
    }
    if (threads < 1) {
        threads = 1;
    }
    if (threads > 256) {
        threads = 256;
    }
    av_log_set_level(AV_LOG_ERROR);

    ret = avformat_open_input(&fmt, path, NULL, NULL);
    if (ret < 0) {
        set_av_err(err, err_len, "FFmpeg 無法開啟檔案", ret);
        return -1;
    }
    ret = avformat_find_stream_info(fmt, NULL);
    if (ret < 0 && fmt->nb_streams == 0) {
        set_av_err(err, err_len, "FFmpeg 無法讀取串流資訊", ret);
        goto fail;
    }

    for (i = 0; i < fmt->nb_streams; i++) {
        if (fmt->streams[i]->codecpar->codec_type != AVMEDIA_TYPE_AUDIO) {
            continue;
        }
        if (audio_seen == stream_ordinal) {
            audio_abs = (int)i;
        }
        audio_seen++;
    }
    info.stream_count = audio_seen;
    info.stream_index = stream_ordinal;
    if (audio_abs < 0) {
        if (audio_seen == 0) {
            set_err(err, err_len, "找不到音訊串流");
        } else if (err && err_len > 0) {
            snprintf(err, (size_t)err_len, "沒有第 %d 個音訊串流（共 %d 個）",
                     stream_ordinal + 1, audio_seen);
        }
        goto fail;
    }

    stream = fmt->streams[audio_abs];
    par = stream->codecpar;
    codec = avcodec_find_decoder(par->codec_id);
    if (!codec) {
        set_err(err, err_len, "FFmpeg 沒有這個格式的解碼器");
        goto fail;
    }
    copy_codec_name(info.codec, sizeof(info.codec), codec);
    ctx = avcodec_alloc_context3(codec);
    if (!ctx || avcodec_parameters_to_context(ctx, par) < 0) {
        set_err(err, err_len, "FFmpeg 無法建立解碼器");
        goto fail;
    }
    ctx->thread_count = threads;
    ctx->thread_type = FF_THREAD_FRAME | FF_THREAD_SLICE;
    if (avcodec_open2(ctx, codec, NULL) < 0) {
        set_err(err, err_len, "FFmpeg 無法建立解碼器");
        goto fail;
    }

    info.sample_rate = ctx->sample_rate > 0 ? ctx->sample_rate : par->sample_rate;
    info.bits_per_sample = par->bits_per_raw_sample ? par->bits_per_raw_sample : par->bits_per_coded_sample;
    if (par->codec_id == AV_CODEC_ID_AAC || par->codec_id == AV_CODEC_ID_WMAV1 ||
        par->codec_id == AV_CODEC_ID_WMAV2 || par->codec_id == AV_CODEC_ID_WMAPRO ||
        par->codec_id == AV_CODEC_ID_MUSEPACK7 || par->codec_id == AV_CODEC_ID_MUSEPACK8) {
        info.bits_per_sample = 0;
    }
    if (par->bit_rate > 0 && par->bit_rate < 0x7fffffff) {
        info.bit_rate = (int)par->bit_rate;
    }
    info.channels = par->ch_layout.nb_channels;
    info.total_samples = estimate_total(fmt, stream, info.sample_rate);
    if (fn(user, NULL, info.channels, 0, &info)) {
        goto cancel;
    }

    packet = av_packet_alloc();
    frame = av_frame_alloc();
    if (!packet || !frame) {
        set_err(err, err_len, "記憶體不足");
        goto fail;
    }

    while ((ret = av_read_frame(fmt, packet)) >= 0) {
        if (packet->stream_index != audio_abs) {
            av_packet_unref(packet);
            continue;
        }
        ret = avcodec_send_packet(ctx, packet);
        av_packet_unref(packet);
        if (ret < 0) {
            continue;
        }
        while ((ret = avcodec_receive_frame(ctx, frame)) >= 0) {
            int stop = emit_frame(frame, fn, user, &info);
            av_frame_unref(frame);
            if (stop == -2) {
                set_err(err, err_len, "FFmpeg 輸出了不支援的取樣格式");
                goto fail;
            }
            if (stop != 0) {
                goto cancel;
            }
            emitted = 1;
        }
    }

    avcodec_send_packet(ctx, NULL);
    while ((ret = avcodec_receive_frame(ctx, frame)) >= 0) {
        int stop = emit_frame(frame, fn, user, &info);
        av_frame_unref(frame);
        if (stop == -2) {
            set_err(err, err_len, "FFmpeg 輸出了不支援的取樣格式");
            goto fail;
        }
        if (stop != 0) {
            goto cancel;
        }
        emitted = 1;
    }

    if (!emitted || info.sample_rate <= 0) {
        set_err(err, err_len, "FFmpeg 沒有解出音訊資料");
        goto fail;
    }
    ret = 0;
    goto cleanup;

cancel:
    ret = 1;
    goto cleanup;
fail:
    ret = -1;
cleanup:
    if (frame) {
        av_frame_free(&frame);
    }
    if (packet) {
        av_packet_free(&packet);
    }
    if (ctx) {
        avcodec_free_context(&ctx);
    }
    if (fmt) {
        avformat_close_input(&fmt);
    }
    return ret;
}
