from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="DeadLetter")


@_attrs_define
class DeadLetter:
    """A cross-app call that was never delivered: its sequence number, the target app and method, why, and when (`atMs`,
    Unix milliseconds).

        Attributes:
            seq (int):
            app (str):
            method (str):
            reason (str):
            at_ms (int):
    """

    seq: int
    app: str
    method: str
    reason: str
    at_ms: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        seq = self.seq

        app = self.app

        method = self.method

        reason = self.reason

        at_ms = self.at_ms

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "seq": seq,
                "app": app,
                "method": method,
                "reason": reason,
                "atMs": at_ms,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        seq = d.pop("seq")

        app = d.pop("app")

        method = d.pop("method")

        reason = d.pop("reason")

        at_ms = d.pop("atMs")

        dead_letter = cls(
            seq=seq,
            app=app,
            method=method,
            reason=reason,
            at_ms=at_ms,
        )

        dead_letter.additional_properties = d
        return dead_letter

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
