from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="FollowUpFailedError")


@_attrs_define
class FollowUpFailedError:
    """
    Attributes:
        status (int):
        error (str):
        message (str):
        details (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for
            whose it is).
    """

    status: int
    error: str
    message: str
    details: Union[Unset, Any] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        status = self.status

        error = self.error

        message = self.message

        details = self.details

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "status": status,
                "error": error,
                "message": message,
            }
        )
        if details is not UNSET:
            field_dict["details"] = details

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        status = d.pop("status")

        error = d.pop("error")

        message = d.pop("message")

        details = d.pop("details", UNSET)

        follow_up_failed_error = cls(
            status=status,
            error=error,
            message=message,
            details=details,
        )

        follow_up_failed_error.additional_properties = d
        return follow_up_failed_error

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
