from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.activity_list_response_200_activity_item_kind import (
    ActivityListResponse200ActivityItemKind,
)
from ..types import UNSET, Unset
from typing import cast, Union
from typing import Union

T = TypeVar("T", bound="ActivityListResponse200ActivityItem")


@_attrs_define
class ActivityListResponse200ActivityItem:
    """
    Attributes:
        at (int):
        passkey (str):
        kind (ActivityListResponse200ActivityItemKind):
        app (Union[None, Unset, str]):
        origin (Union[None, Unset, str]):
        method (Union[Unset, str]):
        subject (Union[Unset, str]):
    """

    at: int
    passkey: str
    kind: ActivityListResponse200ActivityItemKind
    app: Union[None, Unset, str] = UNSET
    origin: Union[None, Unset, str] = UNSET
    method: Union[Unset, str] = UNSET
    subject: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        at = self.at

        passkey = self.passkey

        kind = self.kind.value

        app: Union[None, Unset, str]
        if isinstance(self.app, Unset):
            app = UNSET
        else:
            app = self.app

        origin: Union[None, Unset, str]
        if isinstance(self.origin, Unset):
            origin = UNSET
        else:
            origin = self.origin

        method = self.method

        subject = self.subject

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "at": at,
                "passkey": passkey,
                "kind": kind,
            }
        )
        if app is not UNSET:
            field_dict["app"] = app
        if origin is not UNSET:
            field_dict["origin"] = origin
        if method is not UNSET:
            field_dict["method"] = method
        if subject is not UNSET:
            field_dict["subject"] = subject

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        at = d.pop("at")

        passkey = d.pop("passkey")

        kind = ActivityListResponse200ActivityItemKind(d.pop("kind"))

        def _parse_app(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        app = _parse_app(d.pop("app", UNSET))

        def _parse_origin(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        origin = _parse_origin(d.pop("origin", UNSET))

        method = d.pop("method", UNSET)

        subject = d.pop("subject", UNSET)

        activity_list_response_200_activity_item = cls(
            at=at,
            passkey=passkey,
            kind=kind,
            app=app,
            origin=origin,
            method=method,
            subject=subject,
        )

        activity_list_response_200_activity_item.additional_properties = d
        return activity_list_response_200_activity_item

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
