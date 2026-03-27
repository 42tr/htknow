# Load Testing with 990 files test

## knowledge_base
测试时间：26-03-26
 hey -n 1000 -c 50 -H 'x-user-id: 1' -H 'x-role: admin' http://192.168.0.46:11000/api/v1/knowledge/knowledge_base/

Summary:
  Total:        1.0624 secs
  Slowest:      0.1404 secs
  Fastest:      0.0050 secs
  Average:      0.0501 secs
  Requests/sec: 941.3084

  Total data:   1646000 bytes
  Size/request: 1646 bytes

测试时间：26-03-26
 hey -n 1000 -c 50 -m POST -H 'Content-Type: application/json' -H 'x-user-id: 1' -H 'x-role: admin' -d '{"description": "压测", "is_public": true, "kb_type": "analysis", "name": "压测知识库", "parent_id": 1}' http://192.168.0.46:11000/api/v1/knowledge/knowledge_base/


 hey -n 1000 -c 50 -H 'x-user-id: 1' -H 'x-role: admin' http://192.168.0.46:11000/api/v1/knowledge/knowledge_base/2

Summary:
  Total:        1.9395 secs
  Slowest:      0.3394 secs
  Fastest:      0.0091 secs
  Average:      0.0915 secs
  Requests/sec: 515.5856
