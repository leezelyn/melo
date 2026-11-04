`tone.mp3` 是本次测试自行生成的 1 秒、44.1kHz、双声道 440Hz 正弦波，不含用户歌曲。

生成命令：`ffmpeg -f lavfi -i sine=frequency=440:sample_rate=44100:duration=1 -ac 2 -codec:a libmp3lame -q:a 8 tone.mp3`。
