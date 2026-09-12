import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "generator", Path(__file__).with_name("generate.py")
)
generator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(generator)


class GenerationTests(unittest.TestCase):
    def test_distinct_accounts_scoped_service_and_no_password_reuse(self):
        accounts = {
            "users": [
                {"id": 1, "username": "same", "is_active": True},
                {"id": 2, "username": "same", "is_active": True},
            ],
            "channel_kb_ids": [9],
        }
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "bundle"
            generator.generate(accounts, "cm.internal", output, ["1"])
            realm = json.loads((output / "cm-realm.json").read_text())
            people = [u for u in realm["users"] if "serviceAccountClientId" not in u]
            self.assertEqual(len({u["id"] for u in people}), 2)
            self.assertEqual(len({u["username"] for u in people}), 2)
            self.assertEqual(people[0]["clientRoles"], {"htknow-api": ["htknow-admin"]})
            self.assertEqual(people[1]["clientRoles"], {})
            worker = realm["users"][0]
            self.assertEqual(worker["clientRoles"], {"htknow-api": ["kb-sync"]})
            for u in people:
                self.assertTrue(u["credentials"][0]["temporary"])
            self.assertEqual((output / "oidc.env").stat().st_mode & 0o777, 0o600)
            with self.assertRaises(ValueError):
                generator.generate(accounts, "cm.internal", output, [])


if __name__ == "__main__":
    unittest.main()
