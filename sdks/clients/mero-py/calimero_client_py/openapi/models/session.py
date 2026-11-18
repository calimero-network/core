from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="Session")


@_attrs_define
class Session:
    """A session: its token (15 minutes, by the issuing node) and the refresh token for `/auth/refresh`.

    Attributes:
        account_id (str):
        session_token (str):
        expires_at (int):
        refresh_token (str):
    """

    account_id: str
    session_token: str
    expires_at: int
    refresh_token: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        account_id = self.account_id

        session_token = self.session_token

        expires_at = self.expires_at

        refresh_token = self.refresh_token

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "accountId": account_id,
                "sessionToken": session_token,
                "expiresAt": expires_at,
                "refreshToken": refresh_token,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        account_id = d.pop("accountId")

        session_token = d.pop("sessionToken")

        expires_at = d.pop("expiresAt")

        refresh_token = d.pop("refreshToken")

        session = cls(
            account_id=account_id,
            session_token=session_token,
            expires_at=expires_at,
            refresh_token=refresh_token,
        )

        session.additional_properties = d
        return session

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
