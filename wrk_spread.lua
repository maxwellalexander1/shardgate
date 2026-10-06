-- SPREAD: random key per request -> 16 shards run parallel.
-- wrk -t4 -c64 -d30s --latency -s wrk_spread.lua http://localhost:8080
counter = 0
function request()
    -- math.random per request spreads across 256 users / 16 shards.
    local n = math.random(0, 255)
    return wrk.format(nil, "/check?key=user_" .. n)
end
