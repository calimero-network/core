from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.pipelines_retry_body_limits import PipelinesRetryBodyLimits


T = TypeVar("T", bound="PipelinesRetryBody")


@_attrs_define
class PipelinesRetryBody:
    """
    Attributes:
        limits (Union[Unset, PipelinesRetryBodyLimits]):
        max_cost (Union[Unset, str]):
    """

    limits: Union[Unset, "PipelinesRetryBodyLimits"] = UNSET
    max_cost: Union[Unset, str] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.pipelines_retry_body_limits import PipelinesRetryBodyLimits

        limits: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.limits, Unset):
            limits = self.limits.to_dict()

        max_cost = self.max_cost

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if limits is not UNSET:
            field_dict["limits"] = limits
        if max_cost is not UNSET:
            field_dict["maxCost"] = max_cost

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pipelines_retry_body_limits import PipelinesRetryBodyLimits

        d = dict(src_dict)
        _limits = d.pop("limits", UNSET)
        limits: Union[Unset, PipelinesRetryBodyLimits]
        if isinstance(_limits, Unset):
            limits = UNSET
        else:
            limits = PipelinesRetryBodyLimits.from_dict(_limits)

        max_cost = d.pop("maxCost", UNSET)

        pipelines_retry_body = cls(
            limits=limits,
            max_cost=max_cost,
        )

        return pipelines_retry_body
