from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.follow_up_failed_error import FollowUpFailedError


T = TypeVar("T", bound="FollowUpFailed")


@_attrs_define
class FollowUpFailed:
    """`details` of a `422 FOLLOW_UP_FAILED`: the API key's follow-up that failed (`index` in `then`, its app and method)
    and the error it answered; no key was made.

        Attributes:
            index (int):
            app (str):
            method (str):
            error (FollowUpFailedError):
    """

    index: int
    app: str
    method: str
    error: "FollowUpFailedError"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.follow_up_failed_error import FollowUpFailedError

        index = self.index

        app = self.app

        method = self.method

        error = self.error.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "index": index,
                "app": app,
                "method": method,
                "error": error,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.follow_up_failed_error import FollowUpFailedError

        d = dict(src_dict)
        index = d.pop("index")

        app = d.pop("app")

        method = d.pop("method")

        error = FollowUpFailedError.from_dict(d.pop("error"))

        follow_up_failed = cls(
            index=index,
            app=app,
            method=method,
            error=error,
        )

        follow_up_failed.additional_properties = d
        return follow_up_failed

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
