from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ReleaseDeadline")


@_attrs_define
class ReleaseDeadline:
    """`details` of a `409 RELEASE_DEADLINE`: the mero-tee release whose deadline passed, and the deadline (`after` RFC
    3339, `afterSecs` Unix seconds). Approving that release (or a newer one) lifts it.

        Attributes:
            release (str):
            after (str):
            after_secs (int):
    """

    release: str
    after: str
    after_secs: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        release = self.release

        after = self.after

        after_secs = self.after_secs

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "release": release,
                "after": after,
                "afterSecs": after_secs,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        release = d.pop("release")

        after = d.pop("after")

        after_secs = d.pop("afterSecs")

        release_deadline = cls(
            release=release,
            after=after,
            after_secs=after_secs,
        )

        release_deadline.additional_properties = d
        return release_deadline

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
