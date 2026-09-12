#!/usr/bin/env python3
"""Offline generator: no network, no database changes, no deployment.
Input is the JSON produced by CoAssist/scripts/oidc_accounts.py export.
"""

import argparse
import base64
import json
import os
import secrets
import uuid
from pathlib import Path


def audience(name, target):
    return {
        "name": name,
        "protocol": "openid-connect",
        "protocolMappers": [
            {
                "name": name,
                "protocol": "openid-connect",
                "protocolMapper": "oidc-audience-mapper",
                "config": {
                    "included.client.audience": target,
                    "id.token.claim": "false",
                    "access.token.claim": "true",
                    "introspection.token.claim": "true",
                },
            }
        ],
    }


def generate(accounts, host, output, admin_ids):
    if output.exists():
        raise ValueError(
            "Output already exists; preserve the existing keys and identity bindings"
        )
    if not host or any(
        c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-"
        for c in host
    ):
        raise ValueError(
            "Host must be a DNS name or IPv4 address without scheme or port"
        )
    ids = [str(u["id"]) for u in accounts["users"]]
    if len(ids) != len(set(ids)) or any(not x.isdigit() or int(x) <= 0 for x in ids):
        raise ValueError("User IDs must be unique positive integers")
    if not set(admin_ids).issubset(ids):
        raise ValueError("Every htknow admin ID must exist in the input")
    issuer = f"http://{host}:18080/realms/cm"
    env = {
        "CM_PUBLIC_HOST": host,
        "KEYCLOAK_DB_PASSWORD": secrets.token_urlsafe(32),
        "KEYCLOAK_ADMIN_PASSWORD": secrets.token_urlsafe(32),
        "COASSIST_OIDC_SECRET": secrets.token_urlsafe(32),
        "COASSIST_WORKER_SECRET": secrets.token_urlsafe(32),
        "HTKNOW_OIDC_SECRET": secrets.token_urlsafe(32),
        "HTKNOW_API_SECRET": secrets.token_urlsafe(32),
        "COASSIST_API_SECRET": secrets.token_urlsafe(32),
        "OIDC_TOKEN_ENCRYPTION_KEY": base64.urlsafe_b64encode(
            secrets.token_bytes(32)
        ).decode(),
    }

    def client(name, secret, port=None):
        return {
            "clientId": name,
            "protocol": "openid-connect",
            "enabled": True,
            "publicClient": False,
            "secret": secret,
            "standardFlowEnabled": port is not None,
            "directAccessGrantsEnabled": False,
            "serviceAccountsEnabled": name == "coassist-worker",
            "fullScopeAllowed": True,
            "redirectUris": [
                f"http://{host}:{port}/"
                + (
                    "api/v1/auth/oidc/callback"
                    if name == "coassist"
                    else "api/auth/callback"
                )
            ]
            if port
            else [],
            "webOrigins": [],
            "defaultClientScopes": [
                "profile",
                "roles",
                "coassist-access" if name == "coassist" else "htknow-access",
            ],
            "optionalClientScopes": ["htknow-access"] if name == "coassist" else [],
            "attributes": {
                "pkce.code.challenge.method": "S256",
                "standard.token.exchange.enabled": "true"
                if name == "coassist"
                else "false",
            },
        }

    realm = {
        "realm": "cm",
        "enabled": True,
        "sslRequired": "none",
        "registrationAllowed": False,
        "resetPasswordAllowed": False,
        "bruteForceProtected": True,
        "accessTokenLifespan": 300,
        "ssoSessionIdleTimeout": 28800,
        "ssoSessionMaxLifespan": 28800,
        "revokeRefreshToken": True,
        "refreshTokenMaxReuse": 0,
        "roles": {
            "client": {"htknow-api": [{"name": "htknow-admin"}, {"name": "kb-sync"}]}
        },
        "clientScopes": [
            audience("coassist-access", "coassist-api"),
            audience("htknow-access", "htknow-api"),
        ],
        "clients": [
            client("coassist", env["COASSIST_OIDC_SECRET"], 19080),
            client("htknow", env["HTKNOW_OIDC_SECRET"], 11000),
            client("coassist-worker", env["COASSIST_WORKER_SECRET"]),
            {
                "clientId": "coassist-api",
                "protocol": "openid-connect",
                "publicClient": False,
                "standardFlowEnabled": False,
                "directAccessGrantsEnabled": False,
                "secret": env["COASSIST_API_SECRET"],
            },
            {
                "clientId": "htknow-api",
                "protocol": "openid-connect",
                "publicClient": False,
                "standardFlowEnabled": False,
                "directAccessGrantsEnabled": False,
                "secret": env["HTKNOW_API_SECRET"],
            },
        ],
        "users": [
            {
                "username": "service-account-coassist-worker",
                "enabled": True,
                "serviceAccountClientId": "coassist-worker",
                "clientRoles": {"htknow-api": ["kb-sync"]},
            }
        ],
    }
    # Explicit mappings avoid relying on realm bootstrap defaults when custom scopes exist.
    realm["clientScopes"].append(
        {
            "name": "profile",
            "protocol": "openid-connect",
            "protocolMappers": [
                {
                    "name": "username",
                    "protocol": "openid-connect",
                    "protocolMapper": "oidc-usermodel-property-mapper",
                    "config": {
                        "user.attribute": "username",
                        "claim.name": "preferred_username",
                        "jsonType.label": "String",
                        "id.token.claim": "true",
                        "access.token.claim": "true",
                        "introspection.token.claim": "true",
                    },
                }
            ],
        }
    )
    realm["clientScopes"].append(
        {
            "name": "roles",
            "protocol": "openid-connect",
            "protocolMappers": [
                {
                    "name": "client roles",
                    "protocol": "openid-connect",
                    "protocolMapper": "oidc-usermodel-client-role-mapper",
                    "config": {
                        "claim.name": "resource_access.${client_id}.roles",
                        "jsonType.label": "String",
                        "multivalued": "true",
                        "access.token.claim": "true",
                        "introspection.token.claim": "true",
                    },
                }
            ],
        }
    )
    bindings, credentials = [], []
    for user in accounts["users"]:
        local_id = str(user["id"])
        subject = str(uuid.uuid5(uuid.NAMESPACE_URL, issuer + "/coassist/" + local_id))
        password = secrets.token_urlsafe(18)
        username = (
            "coassist-" + local_id
        )  # Existing CoAssist usernames can be duplicated.
        realm["users"].append(
            {
                "id": subject,
                "username": username,
                "firstName": user.get("full_name") or user["username"],
                "enabled": bool(user["is_active"]),
                "requiredActions": ["UPDATE_PASSWORD"],
                "credentials": [
                    {"type": "password", "value": password, "temporary": True}
                ],
                "clientRoles": {"htknow-api": ["htknow-admin"]}
                if local_id in admin_ids
                else {},
            }
        )
        bindings.append(
            {
                "issuer": issuer,
                "subject": subject,
                "user_id": int(local_id),
                "htknow_user_id": local_id,
            }
        )
        credentials.append(
            {
                "user_id": int(local_id),
                "old_username": user["username"],
                "login": username,
                "temporary_password": password,
            }
        )
    output.mkdir(parents=True, mode=0o700)

    def write(name, value):
        path = output / name
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(value)

    write("cm-realm.json", json.dumps(realm, ensure_ascii=False, indent=2))
    write(
        "bindings.json",
        json.dumps(
            {
                "identities": bindings,
                "channel_kb_ids": accounts.get("channel_kb_ids", []),
            },
            indent=2,
        ),
    )
    write(
        "initial-passwords.json", json.dumps(credentials, ensure_ascii=False, indent=2)
    )
    write("oidc.env", "".join(f"{key}={value}\n" for key, value in env.items()))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("accounts", type=Path)
    parser.add_argument("--host", default="cm.internal")
    parser.add_argument("--output", type=Path, default=Path("keycloak-generated"))
    parser.add_argument("--htknow-admin", action="append", default=[])
    args = parser.parse_args()
    generate(
        json.loads(args.accounts.read_text()), args.host, args.output, args.htknow_admin
    )
    print(f"Generated {args.output}; no services started. Keep this directory private.")
