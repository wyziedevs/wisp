from fastapi import Cookie, FastAPI
from fastapi.responses import HTMLResponse, PlainTextResponse
from jinja2 import Environment

# FastAPI's documented template route is Jinja2; the template is compiled once.
_list = Environment(autoescape=True).from_string(
    "<!DOCTYPE html><html><head></head><body><h1>List</h1><ul>"
    "{% for i in items %}<li>{{ i }}</li>{% endfor %}</ul></body></html>"
)

app = FastAPI(openapi_url=None, docs_url=None, redoc_url=None)


@app.get("/", response_class=PlainTextResponse)
async def index():
    return "Hello, World!"


@app.get("/json")
async def json():
    return {"message": "Hello, World!"}


@app.get("/params/{id}", response_class=PlainTextResponse)
async def params(id: str, q: str = "", sid: str | None = Cookie(None)):
    return f"id={id} q={q} sid={sid if sid is not None else 'none'}"


@app.get("/list", response_class=HTMLResponse)
async def list_():
    return _list.render(items=[f"Item <{i}> & co" for i in range(1, 1001)])


@app.get("/json-big")
async def json_big():
    return [
        {"id": i, "name": f"user-{i}", "active": i % 3 != 0, "score": i * 37 % 101, "tags": ["a", f"t{i % 7}"]}
        for i in range(1, 201)
    ]
