from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.sealed_refusal_error import SealedRefusalError


T = TypeVar("T", bound="SealedRefusal")


@_attrs_define
class SealedRefusal:
    """The sealed channel's own refusal, outside the envelope and in the clear: not an `Error`. `busy` (`503`, with `Retry-
    After: 1`): too many sealed requests or sessions at once, send it again; the other codes (`400`, `malformed` and the
    like) say the envelope could not be opened.

        Attributes:
            error (SealedRefusalError):
    """

    error: "SealedRefusalError"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.sealed_refusal_error import SealedRefusalError

        error = self.error.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "error": error,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.sealed_refusal_error import SealedRefusalError

        d = dict(src_dict)
        error = SealedRefusalError.from_dict(d.pop("error"))

        sealed_refusal = cls(
            error=error,
        )

        sealed_refusal.additional_properties = d
        return sealed_refusal

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
