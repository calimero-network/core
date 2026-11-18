from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.applied_status import AppliedStatus

T = TypeVar("T", bound="Applied")


@_attrs_define
class Applied:
    """An entry an app's log applied: its index, how it ended and the app's error (`null` when it ended `ok`; the app's own
    error value otherwise, free-form). `details` of a `422 UPGRADE_FAILED`.

        Attributes:
            index (int):
            status (AppliedStatus):
            error (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
    """

    index: int
    status: AppliedStatus
    error: Any
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        index = self.index

        status = self.status.value

        error = self.error

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "index": index,
                "status": status,
                "error": error,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        index = d.pop("index")

        status = AppliedStatus(d.pop("status"))

        error = d.pop("error")

        applied = cls(
            index=index,
            status=status,
            error=error,
        )

        applied.additional_properties = d
        return applied

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
