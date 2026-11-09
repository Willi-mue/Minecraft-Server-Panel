"""Run with `python -m app`."""

import logging
import sys

import uvicorn

from . import config

if __name__ == "__main__":
    if not config.PASSWORD:
        sys.exit("ERROR: PANEL_PASSWORD is not set. Set a panel password (e.g. in .env) and start again.")
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(name)s: %(message)s")
    # Graceful-shutdown timeout so open WebSockets can't block `docker stop`
    uvicorn.run(
        "app.main:app",
        host=config.WEB_HOST,
        port=config.WEB_PORT,
        timeout_graceful_shutdown=10,
    )
