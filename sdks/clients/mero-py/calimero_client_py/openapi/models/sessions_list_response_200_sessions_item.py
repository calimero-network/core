from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="SessionsListResponse200SessionsItem")


@_attrs_define
class SessionsListResponse200SessionsItem:
    """
    Attributes:
        session (str):
        passkey (Union[None, str]):
        app (Union[None, str]):
        origin (Union[None, str]):
        opened_at (Union[None, int]):
        last_active_at (int):
        current (bool):
    """

    session: str
    passkey: Union[None, str]
    app: Union[None, str]
    origin: Union[None, str]
    opened_at: Union[None, int]
    last_active_at: int
    current: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        session = self.session

        passkey: Union[None, str]
        passkey = self.passkey

        app: Union[None, str]
        app = self.app

        origin: Union[None, str]
        origin = self.origin

        opened_at: Union[None, int]
        opened_at = self.opened_at

        last_active_at = self.last_active_at

        current = self.current

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "session": session,
                "passkey": passkey,
                "app": app,
                "origin": origin,
                "openedAt": opened_at,
                "lastActiveAt": last_active_at,
                "current": current,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        session = d.pop("session")

        def _parse_passkey(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        passkey = _parse_passkey(d.pop("passkey"))

        def _parse_app(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        app = _parse_app(d.pop("app"))

        def _parse_origin(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        origin = _parse_origin(d.pop("origin"))

        def _parse_opened_at(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        opened_at = _parse_opened_at(d.pop("openedAt"))

        last_active_at = d.pop("lastActiveAt")

        current = d.pop("current")

        sessions_list_response_200_sessions_item = cls(
            session=session,
            passkey=passkey,
            app=app,
            origin=origin,
            opened_at=opened_at,
            last_active_at=last_active_at,
            current=current,
        )

        sessions_list_response_200_sessions_item.additional_properties = d
        return sessions_list_response_200_sessions_item

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
