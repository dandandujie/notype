#!/usr/bin/env python3
"""
Realtime bridge between NoType (Rust) and doubaoime-asr (Python).

Input (stdin, JSON lines):
  {"type":"audio","pcm_b64":"..."}   # 16kHz/mono s16le PCM bytes, base64-encoded
  {"type":"end"}                     # finish stream

Output (stdout, JSON lines):
  {"type":"ready"}
  {"type":"interim","text":"..."}
  {"type":"final","text":"..."}
  {"type":"error","message":"..."}
"""

from __future__ import annotations

import asyncio
import base64
import contextlib
import json
import os
import sys
from typing import AsyncIterator, Optional


def emit(payload: dict) -> None:
    try:
        sys.stdout.write(json.dumps(payload, ensure_ascii=False) + "\n")
        sys.stdout.flush()
    except BrokenPipeError:
        pass


async def stdin_reader(queue: asyncio.Queue[Optional[bytes]]) -> None:
    loop = asyncio.get_running_loop()
    reader = asyncio.StreamReader()
    protocol = asyncio.StreamReaderProtocol(reader)
    await loop.connect_read_pipe(lambda: protocol, sys.stdin)

    while True:
        line = await reader.readline()
        if not line:
            await queue.put(None)
            return

        line = line.strip()
        if not line:
            continue

        try:
            msg = json.loads(line.decode("utf-8", errors="replace"))
        except Exception as e:
            emit({"type": "error", "message": f"invalid_json: {e}"})
            continue

        mtype = msg.get("type")
        if mtype == "audio":
            b64 = msg.get("pcm_b64", "")
            if not b64:
                continue
            try:
                chunk = base64.b64decode(b64)
            except Exception as e:
                emit({"type": "error", "message": f"invalid_base64: {e}"})
                continue
            await queue.put(chunk)
        elif mtype == "end":
            await queue.put(None)
            return


async def audio_source(queue: asyncio.Queue[Optional[bytes]]) -> AsyncIterator[bytes]:
    while True:
        chunk = await queue.get()
        if chunk is None:
            break
        if chunk:
            yield chunk


async def main() -> int:
    try:
        from doubaoime_asr import ASRConfig, ResponseType, transcribe_realtime
    except Exception as e:
        emit(
            {
                "type": "error",
                "message": (
                    "doubaoime-asr not available. Install with "
                    "`pip install doubaoime-asr`. "
                    f"detail={e}"
                ),
            }
        )
        return 2

    config_kwargs = {}
    credential_path = os.getenv("DOUBAO_IME_CREDENTIAL_PATH", "").strip()
    if credential_path:
        config_kwargs["credential_path"] = credential_path

    device_id = os.getenv("DOUBAO_IME_DEVICE_ID", "").strip()
    token = os.getenv("DOUBAO_IME_TOKEN", "").strip()
    if device_id:
        config_kwargs["device_id"] = device_id
    if token:
        config_kwargs["token"] = token

    try:
        config = ASRConfig(**config_kwargs)
    except Exception as e:
        emit({"type": "error", "message": f"invalid_asr_config: {e}"})
        return 2

    queue: asyncio.Queue[Optional[bytes]] = asyncio.Queue(maxsize=64)
    reader_task = asyncio.create_task(stdin_reader(queue))
    emit({"type": "ready"})

    last_interim = ""
    try:
        async for resp in transcribe_realtime(audio_source(queue), config=config):
            if resp.type == ResponseType.INTERIM_RESULT:
                text = (resp.text or "").strip()
                if text and text != last_interim:
                    last_interim = text
                    emit({"type": "interim", "text": text})
            elif resp.type == ResponseType.FINAL_RESULT:
                text = (resp.text or "").strip()
                if text:
                    emit({"type": "final", "text": text})
            elif resp.type == ResponseType.ERROR:
                emit({"type": "error", "message": resp.error_msg or "asr_error"})
                return 1
    except Exception as e:
        emit({"type": "error", "message": f"realtime_asr_failed: {e}"})
        return 1
    finally:
        reader_task.cancel()
        with contextlib.suppress(Exception):
            await reader_task

    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(asyncio.run(main()))
    except KeyboardInterrupt:
        raise SystemExit(130)
