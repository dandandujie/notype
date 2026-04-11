#!/usr/bin/env python3
"""
Local OpenAI-compatible ASR gateway for Doubao IME ASR.

This service provides:
- GET /health
- GET /v1/models
- POST /v1/audio/transcriptions
"""

from __future__ import annotations

import logging
import os
from typing import Optional

import uvicorn
from doubaoime_asr import ASRConfig, ResponseType, transcribe_stream
from fastapi import FastAPI, File, Form, Header, HTTPException, UploadFile
from fastapi.responses import JSONResponse, PlainTextResponse


def _env(name: str, default: str) -> str:
    return os.getenv(name, default).strip() or default


HOST = _env("DOUBAO_ASR_HOST", "127.0.0.1")
PORT = int(_env("DOUBAO_ASR_PORT", "8000"))
CREDENTIAL_PATH = _env(
    "DOUBAO_ASR_CREDENTIAL_PATH", "~/.config/doubaoime-asr/credentials.json"
)
GATEWAY_API_KEY = os.getenv("DOUBAO_ASR_API_KEY", "").strip()
LOG_LEVEL = _env("DOUBAO_ASR_LOG_LEVEL", "info").lower()


def _normalize_auth(value: Optional[str]) -> str:
    if not value:
        return ""
    v = value.strip()
    if v.lower().startswith("bearer "):
        return v[7:].strip()
    return v


def _verify_api_key(authorization: Optional[str]) -> None:
    if not GATEWAY_API_KEY:
        return
    incoming = _normalize_auth(authorization)
    if incoming != GATEWAY_API_KEY:
        raise HTTPException(status_code=401, detail="Invalid API key")


app = FastAPI(title="NoType Doubao ASR Gateway", version="0.1.0")
logger = logging.getLogger("notype_doubao_gateway")


@app.get("/health")
async def health():
    return {"status": "ok"}


@app.get("/v1/models")
async def list_models():
    return {
        "object": "list",
        "data": [
            {
                "id": "doubao-asr",
                "object": "model",
                "owned_by": "notype",
            }
        ],
    }


@app.post("/v1/audio/transcriptions")
async def transcriptions(
    file: UploadFile = File(...),
    model: str = Form("doubao-asr"),
    response_format: str = Form("json"),
    authorization: Optional[str] = Header(default=None),
):
    _verify_api_key(authorization)

    data = await file.read()
    if not data:
        if response_format == "text":
            return PlainTextResponse("")
        return JSONResponse({"text": ""})

    config = ASRConfig(credential_path=CREDENTIAL_PATH)
    final_parts = []

    try:
        async for resp in transcribe_stream(data, config=config, realtime=False):
            if resp.type == ResponseType.FINAL_RESULT and resp.text:
                final_parts.append(resp.text)
            elif resp.type == ResponseType.ERROR:
                raise RuntimeError(resp.error_msg or "asr_error")
    except Exception as exc:
        message = str(exc)
        status = 429 if "ExceededConcurrentQuota" in message else 502
        logger.warning("ASR transcribe failed (%s): %s", status, message)
        raise HTTPException(status_code=status, detail=f"ASR failed: {message}") from exc

    text = "".join(final_parts)
    if response_format == "text":
        return PlainTextResponse(text)
    return JSONResponse({"text": text})


if __name__ == "__main__":
    logging.basicConfig(level=getattr(logging, LOG_LEVEL.upper(), logging.INFO))
    logger.info(
        "Starting Doubao ASR gateway on %s:%s with credential_path=%s",
        HOST,
        PORT,
        CREDENTIAL_PATH,
    )
    uvicorn.run(app, host=HOST, port=PORT, log_level=LOG_LEVEL)
