// Raw @morojs/engine, a native HTTP engine for Node: its the-benchmarker
// entry (javascript/morojs-engine), answering only their routes. Each
// cluster.mjs worker binds the port itself (SO_REUSEPORT).
import engine from '@morojs/engine';

// Method indices in the engine's table: GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS, OTHER
const GET = 0;
const POST = 1;
const USER_PREFIX = '/user/';

const server = engine.serve(
    {
        // Reached only by what the engine does not answer itself: GET
        // /user/:id on an engine without parameter routes, and a 404.
        onRequest(reqId, methodIdx, path) {
            if (methodIdx === GET && path.startsWith(USER_PREFIX)) {
                const id = path.slice(USER_PREFIX.length);
                if (id.length > 0 && !id.includes('/')) {
                    engine.respond(reqId, 200, null, id);
                    return;
                }
            }
            engine.respond(reqId, 404, null, null);
        },
        onAborted() {},
        onWritable() {},
    },
    { reusePort: true },
);

// Fixed replies are the engine's static routes, answered without JavaScript,
// and so is /user/:id where the engine has parameter routes (1.1.9 on).
engine.setStaticRoute(server, GET, '/', 200, null, '');
engine.setStaticRoute(server, POST, '/user', 200, null, '');
if (engine.probe().capabilities?.paramRoutes) {
    engine.setParamRoute(server, GET, USER_PREFIX, '', 200, null);
}

engine.listen(server, '127.0.0.1', Number(process.env.PORT));
