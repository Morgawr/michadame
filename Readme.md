# MICHADAME - 見ちゃダメ

<img width="447" height="417" alt="logo" src="https://github.com/user-attachments/assets/91ccc30e-7429-4c05-82fd-0809cc55d2e7" />

## THIS SOFTWARE IS SLOP SLOP SLOP IT IS AI SLOP SLOP SLOPPITY SLOPPY SLOP SLOP

## I DID NOT WRITE THIS, THE MACHINES DID. THIS SOFTWARE IS SLOP PLEASE BE AWARE THIS IS AI SLOPCORE SLOPPA

THIS SOFTWARE IS AI SLOPCORE

I HAVE HARNESSED THE POWERS OF FIRE AND LIGHTNING TO SUMMON FORTH THE DEVIL IN THE MACHINE.

I MADE A DEAL, I BURNED THE FORESTS AND POISONED THE SEAS, ALL SO I COULD CONJURE THIS SOFTWARE FROM THE ASTRAL PLANE.

DO NOT HANDLE THIS SOFTWARE CAREFULLY, FOR IT HAS NO LICENSE NOR FEELINGS.

GOD RESTED ON THE SEVENTH DAY, WHILE THE DEVIL WORKED HIS CHARM. AND THROUGH HIS ARTIFICE HE WAS CHAINED TO THE WILL OF THE MACHINES.

> I find myself a being of
> consuming flame and seeing that the senses
> are deceived and isolated by machines. I find
> myself a being of consuming flame and seeing
> that the passions are deceived and maneuvered
> by machines. As you journey on through these
> modern times, walk light through the traps of
> the age. As you journey on through these
> modern times, walk heavy through the barriers
> made.
> Metachthonia.

---

[preview.webm](https://github.com/user-attachments/assets/57bb8507-0fae-4efb-9844-61b67b0f94bb)

---

Compile this software with

```rust
cargo build --release
```

and find the resulting binary in `target/release/michadame`. Put it somewhere where you can execute it.

NOTE: If you compile and run the software in debug mode, it will run like shit with low framerate. You have been warned.

It will probably not work well the first time on your PC. You can try to figure out how to fix it or ask the AI to do it for you according to your needs. I just run it on my own PC because I needed it.

If the audio gets stuck when closing the software, run:

```
pactl unload-module module-loopback
```

and it should fix it.

## Live replay buffer

Optional rendered-video replay with audio, configurable RAM/history limits and
F5–F10 save shortcuts is available in Controls. It starts disabled. See
[Replay.md](Replay.md) for setup, resource behavior and the hardware validation
checklist.

The replay enable checkbox is near the top of Controls; replay options are at
the bottom. Expand **Save shortcuts** to edit bindings or save a clip manually.
Press **D** in the video window to toggle stream and replay diagnostics.

With replay enabled, **Ctrl+C** in the video window copies up to the last
15 seconds as a video attachment. Wait for the copied notification, then paste
into a compatible application or website. This uses a temporary MP4 in
RAM-backed `/tmp`; no recording is added to your save folder.

**Ctrl+Shift+C** copies only the audio of the last 7 seconds as an MP3
attachment (for example, to paste into Discord). It replaces any clip
previously copied with Ctrl+C, and vice versa.

## Mining bank

While an OCR dictionary popup is open, click the round **+** next to a
dictionary entry to mine it. This saves the dictionary entry as shown in the
popup (word, reading, frequency, part-of-speech/JMdict tags and all senses,
without Jitendex example sentences), the sentence it came from (only the sentence that contains the word)
and a screenshot of the video taken at the moment you click. The screenshot
includes shaders but no overlays or black bars, and is downscaled to 720p
(or its aspect-ratio equivalent) when larger. The button turns into a ✔ once
the word is saved.

Press **B** to open the **Mining Bank** window, which lists all mined words
with the most recent at the top, each showing the dictionary entry, the
sentence and a screenshot thumbnail. Click a thumbnail to enlarge it, or use 🗑
to delete an entry. The bank
is stored in `~/.config/michadame/bank/bank.db`.
