from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.activity_view_kind import ActivityViewKind
from typing import cast, Union

T = TypeVar("T", bound="ActivityView")


@_attrs_define
class ActivityView:
    """One activity entry: a sign-in (its package and origin) or an approval (its method and subject), every key present,
    those its kind does not have `null`.

        Attributes:
            at (int):
            passkey (str):
            kind (ActivityViewKind):
            app (Union[None, str]):
            origin (Union[None, str]):
            method (Union[None, str]):
            subject (Union[None, str]):
    """

    at: int
    passkey: str
    kind: ActivityViewKind
    app: Union[None, str]
    origin: Union[None, str]
    method: Union[None, str]
    subject: Union[None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        at = self.at

        passkey = self.passkey

        kind = self.kind.value

        app: Union[None, str]
        app = self.app

        origin: Union[None, str]
        origin = self.origin

        method: Union[None, str]
        method = self.method

        subject: Union[None, str]
        subject = self.subject

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "at": at,
                "passkey": passkey,
                "kind": kind,
                "app": app,
                "origin": origin,
                "method": method,
                "subject": subject,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        at = d.pop("at")

        passkey = d.pop("passkey")

        kind = ActivityViewKind(d.pop("kind"))

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

        def _parse_method(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        method = _parse_method(d.pop("method"))

        def _parse_subject(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        subject = _parse_subject(d.pop("subject"))

        activity_view = cls(
            at=at,
            passkey=passkey,
            kind=kind,
            app=app,
            origin=origin,
            method=method,
            subject=subject,
        )

        activity_view.additional_properties = d
        return activity_view

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
