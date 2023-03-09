use std::marker::PhantomData;

use reqwest::RequestBuilder;
use serde::de::DeserializeOwned;

pub struct TypedRequestBuilder<T> {
    builder: RequestBuilder,
    _type:   PhantomData<T>,
}

pub struct TypedResponse<T> {
    response: reqwest::Response,
    _type:    PhantomData<T>,
}

impl<T> TypedRequestBuilder<T> {
    pub fn new(builder: RequestBuilder) -> Self {
        Self {
            builder,
            _type: PhantomData,
        }
    }

    pub async fn send(self) -> Result<TypedResponse<T>, reqwest::Error> {
        let response = self.builder.send().await?;

        Ok(TypedResponse {
            response,
            _type: PhantomData,
        })
    }
}

impl<T> TypedResponse<T>
where
    T: DeserializeOwned,
{
    pub async fn json(self) -> Result<T, reqwest::Error> {
        self.response.json().await
    }
}

impl AsRef<RequestBuilder> for TypedRequestBuilder<()> {
    fn as_ref(&self) -> &RequestBuilder {
        &self.builder
    }
}

impl AsMut<RequestBuilder> for TypedRequestBuilder<()> {
    fn as_mut(&mut self) -> &mut RequestBuilder {
        &mut self.builder
    }
}

impl AsRef<reqwest::Response> for TypedResponse<()> {
    fn as_ref(&self) -> &reqwest::Response {
        &self.response
    }
}

impl AsMut<reqwest::Response> for TypedResponse<()> {
    fn as_mut(&mut self) -> &mut reqwest::Response {
        &mut self.response
    }
}

impl<T> From<RequestBuilder> for TypedRequestBuilder<T> {
    fn from(builder: RequestBuilder) -> Self {
        Self::new(builder)
    }
}
