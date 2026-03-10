import asyncio
from mcp.server.sse import SseServerTransport
from starlette.applications import Starlette
from starlette.routing import Route
from axon.mcp.server import server, server as mcp_server
import uvicorn

sse = SseServerTransport("/messages")

async def handle_sse(request):
    async with sse.connect_sse(request.scope, request.receive, request._send) as (read, write):
        await mcp_server.run(read, write, mcp_server.create_initialization_options())

async def handle_messages(request):
    await sse.handle_post_message(request.scope, request.receive, request._send)

app = Starlette(
    routes=[
        Route("/sse", endpoint=handle_sse),
        Route("/messages", endpoint=handle_messages, methods=["POST"]),
    ]
)

if __name__ == "__main__":
    print("🚀 Axon MCP SSE Bridge starting on port 7000...")
    uvicorn.run(app, host="0.0.0.0", port=7000)
