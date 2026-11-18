from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.pipelines_estimate_run_response_200_steps_additional_property import (
        PipelinesEstimateRunResponse200StepsAdditionalProperty,
    )


T = TypeVar("T", bound="PipelinesEstimateRunResponse200Steps")


@_attrs_define
class PipelinesEstimateRunResponse200Steps:
    """ """

    additional_properties: dict[
        str, "PipelinesEstimateRunResponse200StepsAdditionalProperty"
    ] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.pipelines_estimate_run_response_200_steps_additional_property import (
            PipelinesEstimateRunResponse200StepsAdditionalProperty,
        )

        field_dict: dict[str, Any] = {}
        for prop_name, prop in self.additional_properties.items():
            field_dict[prop_name] = prop.to_dict()

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pipelines_estimate_run_response_200_steps_additional_property import (
            PipelinesEstimateRunResponse200StepsAdditionalProperty,
        )

        d = dict(src_dict)
        pipelines_estimate_run_response_200_steps = cls()

        additional_properties = {}
        for prop_name, prop_dict in d.items():
            additional_property = (
                PipelinesEstimateRunResponse200StepsAdditionalProperty.from_dict(
                    prop_dict
                )
            )

            additional_properties[prop_name] = additional_property

        pipelines_estimate_run_response_200_steps.additional_properties = (
            additional_properties
        )
        return pipelines_estimate_run_response_200_steps

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(
        self, key: str
    ) -> "PipelinesEstimateRunResponse200StepsAdditionalProperty":
        return self.additional_properties[key]

    def __setitem__(
        self, key: str, value: "PipelinesEstimateRunResponse200StepsAdditionalProperty"
    ) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
