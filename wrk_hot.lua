-- HOT: every request hits the same key -> one shard queues.
-- wrk -t4 -c64 -d30s --latency -s wrk_hot.lua http://localhost:8080
function request()
    return wrk.format(nil, "/check?key=hot")
end
